//! TTL-backed registry of pools currently under suspected MEV attack.
//!
//! Written by [`super::listener::run_mempool_listener`] whenever
//! [`super::sandwich::SandwichDetector`] raises an alert; read by
//! [`crate::router::RoutePlanner`] on every quote. Mirrors
//! [`crate::bridge::gas_oracle::GasOracle`]'s use of a `moka` TTL cache: a
//! read never blocks on the listener, so a stalled, disconnected, or never-
//! configured mempool listener degrades to "nothing is ever flagged"
//! rather than adding latency or failure modes to live routing.

use super::PoolKey;
use moka::future::Cache;
use std::time::Duration;

/// How long a pool stays flagged high-risk after a suspected front-run /
/// sandwich pattern is observed against it. Long enough to cover the next
/// block or two — where the attack would actually land — short enough that
/// a stale flag doesn't permanently penalize a pool whose one bad moment
/// has long since passed.
pub const HIGH_RISK_TTL: Duration = Duration::from_secs(120);

/// Extra slippage tolerance, in basis points, [`crate::router::RoutePlanner`]
/// adds on top of a route's normal dynamic slippage when its pool is
/// currently flagged high-risk. Widening the tolerance — rather than
/// rejecting the route outright — keeps the trade executable while giving
/// the user's tx enough slack to survive being sandwiched, and the widened
/// `slippage_bps` on the route response is itself the "warn the user"
/// signal the frontend surfaces.
pub const HIGH_RISK_SLIPPAGE_PENALTY_BPS: u32 = 300;

#[derive(Debug, Clone)]
pub struct RiskFlag {
    pub reason: String,
}

#[derive(Debug)]
pub struct PoolRiskRegistry {
    flags: Cache<PoolKey, RiskFlag>,
}

impl PoolRiskRegistry {
    pub fn new() -> Self {
        Self {
            flags: Cache::builder().time_to_live(HIGH_RISK_TTL).build(),
        }
    }

    /// Flags `pool` as high-risk for [`HIGH_RISK_TTL`], overwriting any
    /// existing flag (and restarting its TTL) rather than stacking.
    pub async fn flag(&self, pool: PoolKey, reason: String) {
        self.flags.insert(pool, RiskFlag { reason }).await;
    }

    pub async fn is_high_risk(&self, pool: &PoolKey) -> bool {
        self.flags.get(pool).await.is_some()
    }
}

impl Default for PoolRiskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::Chain;

    #[tokio::test]
    async fn unflagged_pool_is_not_high_risk() {
        let registry = PoolRiskRegistry::new();
        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        assert!(!registry.is_high_risk(&pool).await);
    }

    #[tokio::test]
    async fn flagged_pool_reports_high_risk() {
        let registry = PoolRiskRegistry::new();
        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        registry
            .flag(pool.clone(), "frontrun bid".to_string())
            .await;
        assert!(registry.is_high_risk(&pool).await);
    }

    #[tokio::test]
    async fn flag_is_independent_per_pool() {
        let registry = PoolRiskRegistry::new();
        let flagged = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        let other = PoolKey::new(Chain::Ethereum, "SOL", "USDC");
        registry
            .flag(flagged.clone(), "frontrun bid".to_string())
            .await;
        assert!(registry.is_high_risk(&flagged).await);
        assert!(!registry.is_high_risk(&other).await);
    }
}
