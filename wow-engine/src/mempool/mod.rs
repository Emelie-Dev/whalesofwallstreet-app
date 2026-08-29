//! Real-time Ethereum mempool monitoring for front-running/sandwich
//! defense.
//!
//! [`RouteOption`](crate::router::RouteOption)s are quoted against static
//! (or gently time-decayed) pool state, so the router has no visibility
//! into MEV bots racing to sandwich a user's trade before it lands.
//! [`listener::run_mempool_listener`] closes that gap: it watches the
//! public Ethereum mempool over a WSS feed, decodes pending calls to our
//! bridge contracts and configured DEX routers, and flags a pool as
//! high-risk in [`PoolRiskRegistry`] the moment a suspected front-run
//! pattern is observed against it. [`crate::router::RoutePlanner`] checks
//! that registry on every quote and widens the slippage tolerance for
//! routes through a currently-flagged pool.
//!
//! Ethereum-only by design: Arbitrum (and L2s generally) order transactions
//! through a sequencer rather than an adversarial public mempool, so there
//! is no equivalent mempool-visible front-running surface there to watch.
//!
//! ```text
//! WSS feed ──▶ listener::run_mempool_listener  (auto-reconnecting)
//!                │
//!                ├─▶ decoder::decode_pending_tx   (ABI-decode calldata)
//!                ├─▶ sandwich::SandwichDetector   (sliding-window pattern match)
//!                └─▶ PoolRiskRegistry::flag       (TTL'd, read by RoutePlanner)
//! ```

pub mod decoder;
pub mod listener;
pub mod risk;
pub mod sandwich;

pub use risk::PoolRiskRegistry;

use crate::bridge::Chain;

/// Identifies one asset pair on one chain — the unit [`PoolRiskRegistry`]
/// flags and [`crate::router::RoutePlanner`] checks before quoting a DEX
/// leg.
///
/// Order-independent: `PoolKey::new(chain, "ETH", "USDC")` and
/// `PoolKey::new(chain, "USDC", "ETH")` compare equal, since a front-run
/// targets the pool itself regardless of which side of it the victim's
/// swap trades.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub chain: Chain,
    asset_low: String,
    asset_high: String,
}

impl PoolKey {
    pub fn new(chain: Chain, asset_a: &str, asset_b: &str) -> Self {
        let a = asset_a.to_uppercase();
        let b = asset_b.to_uppercase();
        let (asset_low, asset_high) = if a <= b { (a, b) } else { (b, a) };
        Self {
            chain,
            asset_low,
            asset_high,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_key_is_order_independent() {
        let a = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        let b = PoolKey::new(Chain::Ethereum, "USDC", "ETH");
        assert_eq!(a, b);
    }

    #[test]
    fn pool_key_is_case_insensitive() {
        let a = PoolKey::new(Chain::Ethereum, "eth", "usdc");
        let b = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        assert_eq!(a, b);
    }

    #[test]
    fn pool_key_distinguishes_chains() {
        let a = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        let b = PoolKey::new(Chain::Arbitrum, "ETH", "USDC");
        assert_ne!(a, b);
    }
}
