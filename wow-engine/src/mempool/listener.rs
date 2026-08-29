//! WSS mempool listener: subscribes to pending transactions targeting our
//! watched contracts, decodes them, and flags pools under suspected
//! front-running attack.
//!
//! Runs forever as a background task (see `main.rs`), the same shape as
//! [`crate::router::arbitrage::run_arbitrage_scanner`] and
//! [`crate::cache_sync::run_redis_subscriber`]: entirely read-only from the
//! request-serving path's point of view, so a stalled or reconnecting
//! listener can never add latency to a live `/quote` request — it only
//! ever writes into [`PoolRiskRegistry`], which [`crate::router::RoutePlanner`]
//! reads independently.
//!
//! The subscription uses Alchemy's `alchemy_pendingTransactions` extension
//! (address-filtered, full-transaction) rather than the standard
//! `eth_subscribe("newPendingTransactions")` every provider supports —
//! that server-side address filter is what keeps this listener from
//! having to decode the entire public mempool (see the module doc on
//! `crate::mempool` for the CPU-cost reasoning). A provider that doesn't
//! implement the extension (e.g. Infura) rejects the subscribe request
//! with a JSON-RPC error; [`run_single_connection`] treats that as a
//! connection failure — surfaced through the same `tracing::warn!` the
//! reconnect loop already logs on every other failure — rather than
//! silently listening forever to a stream that will never emit anything.

use super::decoder::{decode_pending_tx, DecodedKind, RawPendingTx};
use super::risk::PoolRiskRegistry;
use super::sandwich::SandwichDetector;
use crate::bridge::Chain;
use crate::config::AppConfig;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// Delay before the first reconnect attempt after a dropped connection.
/// Doubles on each consecutive failure up to [`MAX_RECONNECT_DELAY`], so a
/// flapping provider gets backed off rather than hammered.
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// Entry point spawned as a background task from `main.rs`. A no-op if
/// `mempool_wss_url` isn't configured or no contracts are watched,
/// mirroring how the Redis cache-sync subscriber degrades to single-node
/// mode rather than failing startup when its own config is absent.
pub async fn run_mempool_listener(
    chain: Chain,
    config: Arc<AppConfig>,
    registry: Arc<PoolRiskRegistry>,
) {
    let Some(url) = config.mempool_wss_url.clone() else {
        tracing::info!("MEMPOOL_WSS_URL not set; mempool front-running monitor disabled");
        return;
    };

    let watched = config.watched_mempool_contracts();
    if watched.is_empty() {
        tracing::warn!(
            "no contracts configured for the mempool listener to watch; monitor disabled"
        );
        return;
    }

    run_reconnect_loop(
        &url,
        chain,
        &watched,
        &registry,
        INITIAL_RECONNECT_DELAY,
        MAX_RECONNECT_DELAY,
    )
    .await;
}

/// Drives repeated connection attempts forever, with exponential backoff
/// between failures and a reset back to `initial_delay` after any
/// connection that was successfully established (even if it later dropped)
/// — so backoff only grows for a provider that's failing outright, not one
/// that connects fine but occasionally cycles.
async fn run_reconnect_loop(
    url: &str,
    chain: Chain,
    watched: &[String],
    registry: &Arc<PoolRiskRegistry>,
    initial_delay: Duration,
    max_delay: Duration,
) {
    let mut delay = initial_delay;
    let mut detector = SandwichDetector::new();

    loop {
        match run_single_connection(url, chain, watched, registry, &mut detector).await {
            Ok(()) => {
                tracing::warn!("mempool WSS stream ended; reconnecting");
                delay = initial_delay;
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    delay_secs = delay.as_secs(),
                    "mempool WSS connection failed; retrying"
                );
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(max_delay);
    }
}

/// Connects once, subscribes, and streams decoded events until the
/// connection closes or errors. `Ok(())` means a clean server-side close;
/// `Err` means a connect/protocol failure. Either way the caller reconnects.
async fn run_single_connection(
    url: &str,
    chain: Chain,
    watched: &[String],
    registry: &Arc<PoolRiskRegistry>,
    detector: &mut SandwichDetector,
) -> anyhow::Result<()> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    let subscribe = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_subscribe",
        "params": ["alchemy_pendingTransactions", { "toAddress": watched, "hashesOnly": false }],
    });
    write
        .send(Message::Text(subscribe.to_string().into()))
        .await?;

    // Confirm the subscription actually succeeded before trusting this
    // connection to ever emit a pending tx. A provider that rejects
    // `alchemy_pendingTransactions` returns a JSON-RPC error here instead
    // of a subscription id — without this check that error is just another
    // frame `handle_message` silently can't parse into an `eth_subscription`
    // shape and drops, leaving a connection that looks alive and subscribed
    // but will never see a single transaction.
    let subscription_id = loop {
        match read.next().await.ok_or_else(|| {
            anyhow::anyhow!("connection closed before subscription was acknowledged")
        })?? {
            Message::Text(text) => break parse_subscribe_ack(&text)?,
            Message::Close(_) => {
                anyhow::bail!("connection closed before subscription was acknowledged")
            }
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    };
    tracing::info!(
        contracts = ?watched,
        subscription_id = %subscription_id,
        "mempool listener subscribed to pending transactions"
    );

    while let Some(msg) = read.next().await {
        match msg? {
            Message::Text(text) => handle_message(&text, chain, registry, detector),
            Message::Close(_) => break,
            // Ping/Pong are handled automatically by tokio-tungstenite;
            // Binary/Frame carry nothing this feed ever sends.
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}

/// Parses the JSON-RPC response to the `eth_subscribe` request: `Ok` with
/// the subscription id on success, `Err` on a JSON-RPC error response (a
/// provider rejecting `alchemy_pendingTransactions`) or any other shape
/// that isn't a valid subscription ack.
fn parse_subscribe_ack(text: &str) -> anyhow::Result<String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| anyhow::anyhow!("subscription ack was not valid JSON: {err}"))?;

    if let Some(error) = value.get("error") {
        anyhow::bail!(
            "mempool WSS provider rejected the subscribe request: {error} \
             (alchemy_pendingTransactions is an Alchemy-specific extension; \
             this endpoint may not support it — see MEMPOOL_WSS_URL's doc comment)"
        );
    }

    match value.get("result").and_then(|r| r.as_str()) {
        Some(id) => Ok(id.to_string()),
        None => anyhow::bail!("subscription ack had no result subscription id: {text}"),
    }
}

/// Parses one WSS text frame as an `eth_subscription` notification,
/// decodes its pending tx, and — for a recognized DEX swap — runs it
/// through the sandwich detector, flagging the pool and logging an alert
/// for anything raised. Any parse failure (a subscription confirmation
/// frame, an unrelated notification shape, malformed JSON) is silently
/// ignored: this is a best-effort monitor layered on top of routing, never
/// a dependency it can fail.
fn handle_message(
    text: &str,
    chain: Chain,
    registry: &Arc<PoolRiskRegistry>,
    detector: &mut SandwichDetector,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(result) = value.get("params").and_then(|p| p.get("result")) else {
        return;
    };
    let Ok(raw) = serde_json::from_value::<RawPendingTx>(result.clone()) else {
        return;
    };
    let Some(decoded) = decode_pending_tx(chain, &raw) else {
        return;
    };

    match decoded.kind.clone() {
        DecodedKind::BridgeCall { contract } => {
            tracing::info!(
                hash = %decoded.hash,
                contract = %contract,
                "pending transaction targeting a bridge contract observed in mempool"
            );
        }
        DecodedKind::DexSwap { pool } => {
            for alert in detector.observe(decoded.clone(), pool.clone()) {
                tracing::warn!(
                    pool = ?alert.pool,
                    attacker = %alert.attacker,
                    victim_hash = %alert.victim_hash,
                    pattern = ?alert.pattern,
                    "SandwichAlert: suspected front-run pattern detected in mempool"
                );
                registry.flag(
                    alert.pool.clone(),
                    format!("{:?} via tx {}", alert.pattern, decoded.hash),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempool::PoolKey;
    use futures_util::SinkExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    /// Minimal Alchemy-shaped pending-tx notification carrying a swap that
    /// decodes successfully (matches the fixtures in `decoder`'s tests).
    fn swap_notification(hash: &str, from: &str, gas_price_wei_hex: &str) -> String {
        // swapExactTokensForTokens(1000, 1, [WETH, USDC], to, 0)
        let selector = "38ed1739"; // keccak256("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)")[..4]
        let weth = "000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2";
        let usdc = "000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
        let input = format!(
            "0x{selector}\
             00000000000000000000000000000000000000000000000000000000000003e8\
             0000000000000000000000000000000000000000000000000000000000000001\
             00000000000000000000000000000000000000000000000000000000000000a0\
             0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000000000000000000000000000000000000000000000000002\
             {weth}\
             {usdc}"
        );
        format!(
            r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0xsub","result":{{"hash":"{hash}","from":"{from}","to":"0xRouter","input":"{input}","gasPrice":"{gas_price_wei_hex}"}}}}}}"#
        )
    }

    #[test]
    fn handle_message_flags_a_pool_on_a_detected_frontrun() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();

        let victim = swap_notification("0xvictim", "0xalice", "0x4a817c800"); // 20 gwei
        handle_message(&victim, Chain::Ethereum, &registry, &mut detector);

        let attacker = swap_notification("0xattack", "0xbot", "0x9502f9000"); // 40 gwei
        handle_message(&attacker, Chain::Ethereum, &registry, &mut detector);

        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        assert!(registry.is_high_risk(&pool));
    }

    #[test]
    fn parse_subscribe_ack_accepts_a_valid_result() {
        let id = parse_subscribe_ack(r#"{"jsonrpc":"2.0","id":1,"result":"0xabc123"}"#).unwrap();
        assert_eq!(id, "0xabc123");
    }

    #[test]
    fn parse_subscribe_ack_rejects_a_json_rpc_error() {
        // The exact shape a provider that doesn't implement Alchemy's
        // alchemy_pendingTransactions extension (e.g. Infura) returns.
        let err = parse_subscribe_ack(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("rejected"));
    }

    #[test]
    fn parse_subscribe_ack_rejects_malformed_json() {
        assert!(parse_subscribe_ack("not json").is_err());
    }

    #[test]
    fn parse_subscribe_ack_rejects_a_response_with_no_result() {
        assert!(parse_subscribe_ack(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
    }

    #[tokio::test]
    async fn subscribe_rejection_surfaces_as_a_connection_error_not_silence() {
        // Regression test for the exact failure mode a JSON-RPC error to
        // the subscribe request used to produce: run_single_connection
        // would enter its read loop anyway, handle_message would silently
        // drop the error frame (it doesn't parse as an eth_subscription
        // notification), and the listener would sit there "connected" and
        // "subscribed" forever without ever seeing a pending transaction.
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let url = format!("ws://{addr}");

        tokio::spawn(async move {
            let (stream, _) = tcp.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.next().await; // the subscribe request
            let _ = ws
                .send(Message::Text(
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#
                        .into(),
                ))
                .await;
        });

        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();
        let result = run_single_connection(
            &url,
            Chain::Ethereum,
            &["0xdeadbeef".to_string()],
            &registry,
            &mut detector,
        )
        .await;

        assert!(
            result.is_err(),
            "a rejected subscribe must surface as a connection error, not a silent no-op listener"
        );
    }

    #[test]
    fn handle_message_ignores_a_subscription_confirmation() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();
        let confirmation = r#"{"jsonrpc":"2.0","id":1,"result":"0xsubscriptionid"}"#;
        // Must not panic on a message shape with no `params.result`.
        handle_message(confirmation, Chain::Ethereum, &registry, &mut detector);
    }

    #[test]
    fn handle_message_ignores_malformed_json() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();
        handle_message("not json at all", Chain::Ethereum, &registry, &mut detector);
    }

    #[tokio::test]
    async fn reconnects_after_the_server_drops_the_connection() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let url = format!("ws://{addr}");

        let connect_count = Arc::new(AtomicU32::new(0));
        let connect_count_srv = connect_count.clone();

        // Serves exactly two connections: reads the subscribe request off
        // each, then closes — simulating the provider dropping the socket.
        // A real reconnect must dial the listener a second time.
        tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = tcp.accept().await.unwrap();
                connect_count_srv.fetch_add(1, Ordering::SeqCst);
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let _ = ws.next().await;
                let _ = ws.send(Message::Close(None)).await;
            }
            // Keep the listener alive so a third connect attempt (if the
            // test's timeout is generous) doesn't error the test task out.
            std::future::pending::<()>().await;
        });

        let registry = Arc::new(PoolRiskRegistry::new());
        let watched = vec!["0xdeadbeef".to_string()];
        let run = run_reconnect_loop(
            &url,
            Chain::Ethereum,
            &watched,
            &registry,
            Duration::from_millis(5),
            Duration::from_millis(20),
        );
        let _ = tokio::time::timeout(Duration::from_secs(2), run).await;

        assert_eq!(connect_count.load(Ordering::SeqCst), 2);
    }
}
