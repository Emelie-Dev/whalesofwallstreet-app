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
    tracing::info!(contracts = ?watched, "mempool listener subscribed to pending transactions");

    while let Some(msg) = read.next().await {
        match msg? {
            Message::Text(text) => handle_message(&text, chain, registry, detector).await,
            Message::Close(_) => break,
            // Ping/Pong are handled automatically by tokio-tungstenite;
            // Binary/Frame carry nothing this feed ever sends.
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}

/// Parses one WSS text frame as an `eth_subscription` notification,
/// decodes its pending tx, and — for a recognized DEX swap — runs it
/// through the sandwich detector, flagging the pool and logging an alert
/// for anything raised. Any parse failure (a subscription confirmation
/// frame, an unrelated notification shape, malformed JSON) is silently
/// ignored: this is a best-effort monitor layered on top of routing, never
/// a dependency it can fail.
async fn handle_message(
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
                registry
                    .flag(
                        alert.pool.clone(),
                        format!("{:?} via tx {}", alert.pattern, decoded.hash),
                    )
                    .await;
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

    #[tokio::test]
    async fn handle_message_flags_a_pool_on_a_detected_frontrun() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();

        let victim = swap_notification("0xvictim", "0xalice", "0x4a817c800"); // 20 gwei
        handle_message(&victim, Chain::Ethereum, &registry, &mut detector).await;

        let attacker = swap_notification("0xattack", "0xbot", "0x9502f9000"); // 40 gwei
        handle_message(&attacker, Chain::Ethereum, &registry, &mut detector).await;

        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        assert!(registry.is_high_risk(&pool).await);
    }

    #[tokio::test]
    async fn handle_message_ignores_a_subscription_confirmation() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();
        let confirmation = r#"{"jsonrpc":"2.0","id":1,"result":"0xsubscriptionid"}"#;
        // Must not panic on a message shape with no `params.result`.
        handle_message(confirmation, Chain::Ethereum, &registry, &mut detector).await;
    }

    #[tokio::test]
    async fn handle_message_ignores_malformed_json() {
        let registry = Arc::new(PoolRiskRegistry::new());
        let mut detector = SandwichDetector::new();
        handle_message("not json at all", Chain::Ethereum, &registry, &mut detector).await;
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
