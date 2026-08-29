//! TTL-backed registry of pools currently under suspected MEV attack.
//!
//! Written by [`super::listener::run_mempool_listener`] whenever
//! [`super::sandwich::SandwichDetector`] raises an alert; read by
//! [`crate::router::RoutePlanner`] on every quote — including inside the
//! Dijkstra search's inner loop, which runs once per candidate edge
//! explored, not once per request. Unlike
//! [`crate::bridge::gas_oracle::GasOracle`] (which uses `moka::future`
//! because its cache is fronting a real network fetch via
//! `try_get_with`), this registry never does I/O: `flag`/`is_high_risk`
//! are pure in-memory operations, so `moka::sync` is the right tool —
//! it drops the async-runtime overhead an `await` on every explored edge
//! would otherwise add to the routing hot path.

use super::PoolKey;
use moka::sync::Cache;
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
    pub fn flag(&self, pool: PoolKey, reason: String) {
        self.flags.insert(pool, RiskFlag { reason });
        // `entry_count()` (used by `is_empty`) only reflects moka's
        // internal counters as of its last housekeeping pass — without
        // this, a pool flagged an instant ago could still read as "empty"
        // to a concurrent `is_empty` call, silently skipping the one
        // lookup that would have found it. `flag` is called rarely (only
        // when the listener raises an alert), so paying for an immediate
        // housekeeping pass here is cheap insurance for a read path that
        // runs on every candidate edge in the router's hot loop.
        self.flags.run_pending_tasks();
    }

    /// Cheap, allocation-free pre-check for the common case (no mempool
    /// listener running, or one that simply hasn't flagged anything
    /// recently): `true` when the registry is certainly empty, via moka's
    /// O(1) approximate entry count rather than a real lookup.
    ///
    /// Callers on the routing hot path should branch on this *before*
    /// building a [`PoolKey`] to look up — [`PoolKey::new`] allocates two
    /// `String`s, so skipping straight past it when there's nothing to
    /// find avoids that allocation on every candidate edge, not just the
    /// cache read.
    pub fn is_empty(&self) -> bool {
        self.flags.entry_count() == 0
    }

    pub fn is_high_risk(&self, pool: &PoolKey) -> bool {
        self.flags.contains_key(pool)
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

    #[test]
    fn unflagged_pool_is_not_high_risk() {
        let registry = PoolRiskRegistry::new();
        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        assert!(!registry.is_high_risk(&pool));
        assert!(registry.is_empty());
    }

    #[test]
    fn flagged_pool_reports_high_risk() {
        let registry = PoolRiskRegistry::new();
        let pool = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        registry.flag(pool.clone(), "frontrun bid".to_string());
        assert!(registry.is_high_risk(&pool));
        assert!(!registry.is_empty());
    }

    #[test]
    fn flag_is_independent_per_pool() {
        let registry = PoolRiskRegistry::new();
        let flagged = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        let other = PoolKey::new(Chain::Ethereum, "SOL", "USDC");
        registry.flag(flagged.clone(), "frontrun bid".to_string());
        assert!(registry.is_high_risk(&flagged));
        assert!(!registry.is_high_risk(&other));
    }
}
