//! Redis Pub/Sub based cache invalidation for multi-node deployments.
//!
//! When the engine runs as a single process, [`crate::bridge::gas_oracle::GasOracle`]'s
//! local Moka cache and its own TTL are all that's needed for consistency. Once
//! the engine is horizontally scaled, each node keeps its own independent
//! cache: a critical state change (e.g. an admin invalidating stale pricing
//! data) observed on one node is invisible to the others until their TTLs
//! expire, which can leave the cluster quoting bad routes for a window of
//! time.
//!
//! This module closes that gap: a node that detects a critical state change
//! calls [`CacheInvalidationBroadcaster::publish`] to broadcast an
//! [`InvalidationMessage`] over a shared Redis channel, and every node runs
//! [`run_redis_subscriber`] in the background to receive that message and
//! evict the matching entry from its own local cache immediately.
//!
//! Redis is treated as a pure optimization, never a hard dependency: if it is
//! unset, unreachable, or drops mid-stream, publishing becomes a silent
//! no-op and the subscriber reconnects with backoff in the background —
//! nodes simply fall back to relying on the cache's own TTL, exactly as they
//! do today.

use crate::bridge::gas_oracle::GasOracle;
use crate::bridge::Chain;
use redis::aio::{ConnectionManager, PubSub};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Redis channel every node publishes to and subscribes on for cache
/// invalidation broadcasts.
pub const CACHE_INVALIDATION_CHANNEL: &str = "wow_engine:cache_invalidate";

const MIN_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);

/// Wire format for a cluster-wide cache invalidation event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvalidationMessage {
    /// Evict the cached entry for a single chain (e.g. its gas/liquidity data
    /// is known stale on the node that published this message).
    InvalidateChain { chain: Chain },
    /// Evict every cached entry, regardless of chain (e.g. an admin-triggered
    /// emergency pause that invalidates all currently quoted state).
    InvalidateAll,
}

/// Abstraction over "a thing invalidation messages can be published to".
///
/// Exists so [`CacheInvalidationBroadcaster`] can be exercised in unit tests
/// against a mock transport, without opening a real network connection to
/// Redis. [`redis::aio::ConnectionManager`] is the production implementation.
pub trait RedisPublish: Send + Sync {
    fn publish_message(
        &self,
        channel: &str,
        payload: String,
    ) -> impl Future<Output = redis::RedisResult<()>> + Send;
}

impl RedisPublish for ConnectionManager {
    async fn publish_message(&self, channel: &str, payload: String) -> redis::RedisResult<()> {
        // ConnectionManager is cheap to clone (it shares the underlying
        // multiplexed connection) and its command methods take `&mut self`,
        // so we clone it here rather than requiring `&mut self` up our own
        // call chain.
        let mut conn = self.clone();
        conn.publish(channel, payload).await
    }
}

/// Publishes cache invalidation events to the cluster, degrading gracefully
/// when Redis is unavailable.
///
/// `transport` is `None` when `REDIS_URL` was never configured or the
/// initial connection attempt failed at startup; in that case every call to
/// [`publish`](Self::publish) is a logged no-op rather than an error, so
/// callers never need special-case "Redis is down" handling.
pub struct CacheInvalidationBroadcaster<P: RedisPublish> {
    transport: Option<P>,
    channel: String,
}

impl<P: RedisPublish> CacheInvalidationBroadcaster<P> {
    pub fn new(transport: Option<P>, channel: impl Into<String>) -> Self {
        Self {
            transport,
            channel: channel.into(),
        }
    }

    /// Broadcasts `message` to every other node in the cluster.
    ///
    /// Best-effort: a missing transport, a serialization failure, or a Redis
    /// error are all logged and swallowed rather than propagated, since a
    /// failed broadcast must never fail (or crash) the caller's request —
    /// the node that published still applies the invalidation locally, and
    /// other nodes fall back to their own TTL.
    pub async fn publish(&self, message: &InvalidationMessage) {
        let Some(transport) = &self.transport else {
            tracing::debug!(
                "Redis not configured; skipping cluster-wide cache invalidation broadcast"
            );
            return;
        };

        let payload = match serde_json::to_string(message) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::warn!("Failed to serialize cache invalidation message: {err}");
                return;
            }
        };

        if let Err(err) = transport.publish_message(&self.channel, payload).await {
            tracing::warn!(
                "Failed to publish cache invalidation to Redis (nodes will fall back to local TTLs \
                 for this entry): {err}"
            );
        }
    }
}

/// Production broadcaster type, wired up from a real Redis connection.
pub type RedisBroadcaster = CacheInvalidationBroadcaster<ConnectionManager>;

/// Abstraction over "a stream of raw invalidation payloads received from
/// Redis pub/sub", so the message-handling loop can be unit tested without a
/// real Redis subscription.
pub trait InvalidationSource: Send {
    /// Returns the next message payload, or `None` when the underlying
    /// stream has ended (e.g. the Redis connection dropped), signalling the
    /// caller should reconnect.
    fn next_payload(&mut self) -> impl Future<Output = Option<String>> + Send;
}

/// Real Redis-backed [`InvalidationSource`], wrapping a subscribed [`PubSub`]
/// connection.
pub struct RedisInvalidationSource {
    pubsub: PubSub,
}

impl InvalidationSource for RedisInvalidationSource {
    async fn next_payload(&mut self) -> Option<String> {
        use futures_util::StreamExt;
        let msg = self.pubsub.on_message().next().await?;
        match msg.get_payload::<String>() {
            Ok(payload) => Some(payload),
            Err(err) => {
                tracing::warn!("Ignoring unreadable Redis pub/sub payload: {err}");
                // Keep consuming the stream rather than tearing the
                // connection down over one malformed frame.
                Some(String::new())
            }
        }
    }
}

/// Applies a single invalidation payload to `gas_oracle`.
///
/// A malformed payload (unknown shape, bad JSON, or the empty placeholder
/// emitted for an unreadable frame) is logged and ignored rather than
/// treated as fatal — one bad message must never take down the subscriber
/// loop for the whole node.
async fn apply_invalidation_payload(payload: &str, gas_oracle: &GasOracle) {
    if payload.is_empty() {
        return;
    }

    match serde_json::from_str::<InvalidationMessage>(payload) {
        Ok(InvalidationMessage::InvalidateChain { chain }) => {
            gas_oracle.invalidate(chain).await;
            tracing::info!(?chain, "Evicted local cache entry via cluster invalidation");
        }
        Ok(InvalidationMessage::InvalidateAll) => {
            gas_oracle.invalidate_all().await;
            tracing::info!("Evicted entire local cache via cluster invalidation");
        }
        Err(err) => {
            tracing::warn!("Ignoring malformed cache invalidation message: {err}");
        }
    }
}

/// Drains `source` until it ends, applying every message to `gas_oracle`.
///
/// Returns when `source` reports no more messages (e.g. the connection was
/// lost), so the caller can decide how to reconnect.
pub async fn run_invalidation_loop<S: InvalidationSource>(
    mut source: S,
    gas_oracle: Arc<GasOracle>,
) {
    while let Some(payload) = source.next_payload().await {
        apply_invalidation_payload(&payload, &gas_oracle).await;
    }
}

/// Doubles `current`, capped at [`MAX_RECONNECT_BACKOFF`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_RECONNECT_BACKOFF)
}

async fn connect_and_subscribe(redis_url: &str, channel: &str) -> redis::RedisResult<PubSub> {
    let client = redis::Client::open(redis_url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    Ok(pubsub)
}

/// Runs forever, keeping every node's [`GasOracle`] in sync with
/// cluster-wide invalidation events.
///
/// If `redis_url` is `None`, this returns immediately: the node runs in
/// single-node mode and relies purely on local TTLs, exactly as it did
/// before this module existed. If Redis is configured but unreachable (or
/// becomes unreachable later), the loop logs a warning and retries with
/// exponential backoff — it never panics and never blocks request handling,
/// since it always runs as its own background task.
pub async fn run_redis_subscriber(redis_url: Option<String>, gas_oracle: Arc<GasOracle>) {
    let Some(redis_url) = redis_url else {
        tracing::info!(
            "REDIS_URL not set; running in single-node mode with local TTL-only caching"
        );
        return;
    };

    let mut backoff = MIN_RECONNECT_BACKOFF;
    loop {
        match connect_and_subscribe(&redis_url, CACHE_INVALIDATION_CHANNEL).await {
            Ok(pubsub) => {
                tracing::info!(
                    "Subscribed to Redis channel '{CACHE_INVALIDATION_CHANNEL}' for cluster cache invalidation"
                );
                backoff = MIN_RECONNECT_BACKOFF;
                let source = RedisInvalidationSource { pubsub };
                run_invalidation_loop(source, gas_oracle.clone()).await;
                tracing::warn!("Redis pub/sub connection dropped; reconnecting");
            }
            Err(err) => {
                tracing::warn!(
                    "Could not reach Redis for cache invalidation ({err}); falling back to local \
                     TTL-only caching, retrying in {backoff:?}"
                );
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

/// Bundles the shared cache primitives a request handler needs: the local
/// cache itself, plus (optionally) the means to tell the rest of the cluster
/// about an invalidation. Cloning is cheap — both fields are `Arc`s.
#[derive(Clone)]
pub struct ClusterCache {
    pub gas_oracle: Arc<GasOracle>,
    pub broadcaster: Option<Arc<RedisBroadcaster>>,
}

impl ClusterCache {
    /// Builds a cache with no cluster connectivity: purely local, TTL-only.
    /// Used as the default for callers (and tests) that don't need Redis.
    pub fn local_only() -> Self {
        Self {
            gas_oracle: Arc::new(GasOracle::new()),
            broadcaster: None,
        }
    }

    /// Invalidates `message` locally and best-effort broadcasts it to the
    /// rest of the cluster.
    pub async fn invalidate(&self, message: InvalidationMessage) {
        match message {
            InvalidationMessage::InvalidateChain { chain } => {
                self.gas_oracle.invalidate(chain).await;
            }
            InvalidationMessage::InvalidateAll => {
                self.gas_oracle.invalidate_all().await;
            }
        }

        if let Some(broadcaster) = &self.broadcaster {
            broadcaster.publish(&message).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Mock Redis publish transport: records every publish call and can be
    /// configured to fail, standing in for "Redis connection is down".
    struct MockPublisher {
        calls: Mutex<Vec<(String, String)>>,
        should_fail: bool,
    }

    impl MockPublisher {
        fn new(should_fail: bool) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                should_fail,
            }
        }
    }

    impl RedisPublish for MockPublisher {
        async fn publish_message(&self, channel: &str, payload: String) -> redis::RedisResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push((channel.to_string(), payload));
            if self.should_fail {
                Err(redis::RedisError::from(std::io::Error::other(
                    "mock redis connection refused",
                )))
            } else {
                Ok(())
            }
        }
    }

    /// Mock Redis pub/sub subscription: yields a fixed, pre-recorded queue of
    /// payloads, standing in for messages received over the wire.
    struct MockInvalidationSource {
        queue: VecDeque<String>,
    }

    impl InvalidationSource for MockInvalidationSource {
        async fn next_payload(&mut self) -> Option<String> {
            self.queue.pop_front()
        }
    }

    #[test]
    fn invalidation_message_round_trips_through_json() {
        let msg = InvalidationMessage::InvalidateChain {
            chain: Chain::Ethereum,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: InvalidationMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);

        let msg = InvalidationMessage::InvalidateAll;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: InvalidationMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn broadcaster_skips_publish_when_transport_is_none() {
        let broadcaster: CacheInvalidationBroadcaster<MockPublisher> =
            CacheInvalidationBroadcaster::new(None, CACHE_INVALIDATION_CHANNEL);

        // Must not panic and must simply return; there's no transport to assert on.
        broadcaster
            .publish(&InvalidationMessage::InvalidateAll)
            .await;
    }

    #[tokio::test]
    async fn broadcaster_publishes_correct_channel_and_payload() {
        let transport = MockPublisher::new(false);
        let broadcaster = CacheInvalidationBroadcaster::new(Some(transport), "test-channel");

        broadcaster
            .publish(&InvalidationMessage::InvalidateChain {
                chain: Chain::Solana,
            })
            .await;

        let calls = broadcaster
            .transport
            .as_ref()
            .unwrap()
            .calls
            .lock()
            .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "test-channel");
        assert_eq!(
            calls[0].1,
            serde_json::to_string(&InvalidationMessage::InvalidateChain {
                chain: Chain::Solana
            })
            .unwrap()
        );
    }

    #[tokio::test]
    async fn broadcaster_swallows_publish_errors() {
        let transport = MockPublisher::new(true);
        let broadcaster = CacheInvalidationBroadcaster::new(Some(transport), "test-channel");

        // Redis being down must not surface as an error to the caller.
        broadcaster
            .publish(&InvalidationMessage::InvalidateAll)
            .await;

        let calls = broadcaster
            .transport
            .as_ref()
            .unwrap()
            .calls
            .lock()
            .unwrap();
        assert_eq!(calls.len(), 1, "the attempt should still have been made");
    }

    #[tokio::test]
    async fn subscriber_loop_processes_multiple_messages_in_order() {
        let gas_oracle = Arc::new(GasOracle::new());
        gas_oracle
            .cache_insert_for_test(Chain::Ethereum, 99.0)
            .await;
        gas_oracle.cache_insert_for_test(Chain::Solana, 5.0).await;

        let source = MockInvalidationSource {
            queue: VecDeque::from([
                serde_json::to_string(&InvalidationMessage::InvalidateChain {
                    chain: Chain::Ethereum,
                })
                .unwrap(),
                serde_json::to_string(&InvalidationMessage::InvalidateChain {
                    chain: Chain::Solana,
                })
                .unwrap(),
            ]),
        };

        run_invalidation_loop(source, gas_oracle.clone()).await;

        assert_eq!(
            gas_oracle.cached_value_for_test(Chain::Ethereum).await,
            None
        );
        assert_eq!(gas_oracle.cached_value_for_test(Chain::Solana).await, None);
    }

    #[tokio::test]
    async fn malformed_message_is_ignored_without_panicking() {
        let gas_oracle = Arc::new(GasOracle::new());
        gas_oracle
            .cache_insert_for_test(Chain::Ethereum, 99.0)
            .await;

        apply_invalidation_payload("{not valid json", &gas_oracle).await;
        apply_invalidation_payload("", &gas_oracle).await;

        // Untouched: the malformed messages must not have evicted anything.
        assert_eq!(
            gas_oracle.cached_value_for_test(Chain::Ethereum).await,
            Some(99.0)
        );
    }

    #[test]
    fn backoff_doubles_and_caps_at_max() {
        let mut backoff = MIN_RECONNECT_BACKOFF;
        for _ in 0..10 {
            backoff = next_backoff(backoff);
        }
        assert_eq!(backoff, MAX_RECONNECT_BACKOFF);
    }

    #[tokio::test]
    async fn cluster_cache_local_only_invalidates_without_a_broadcaster() {
        let cache = ClusterCache::local_only();
        cache
            .gas_oracle
            .cache_insert_for_test(Chain::Ethereum, 1.0)
            .await;

        cache
            .invalidate(InvalidationMessage::InvalidateChain {
                chain: Chain::Ethereum,
            })
            .await;

        assert_eq!(
            cache
                .gas_oracle
                .cached_value_for_test(Chain::Ethereum)
                .await,
            None
        );
    }
}
