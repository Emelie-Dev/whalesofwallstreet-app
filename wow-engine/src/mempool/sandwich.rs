//! Sliding-window sandwich/front-run pattern matching over decoded pending
//! swaps.
//!
//! We only ever observe *pending* (unconfirmed) transactions, never the
//! block they land in, so a true sandwich (attacker front-run, victim,
//! attacker back-run, all three mined in order) can't be confirmed from
//! mempool data alone. What the mempool *does* reveal, in real time and
//! before it's too late to matter, is the setup: a bot racing to get
//! ahead of a pending swap by outbidding it on gas, or the same address
//! bracketing someone else's pending swap on the same pool. Both are
//! flagged here, since both are exactly the setup a wider slippage
//! tolerance protects against.
//!
//! This is a plain, single-threaded struct — no locking — because exactly
//! one task (the listener) ever drives it, off the single stream of
//! decoded events from one WSS connection.

use super::decoder::DecodedPendingTx;
use super::PoolKey;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long an observed pending swap stays "live" for pattern matching
/// against subsequently observed swaps on the same pool. Roughly one
/// Ethereum block (~12s) plus slack: a front-run's bracketing legs are
/// broadcast within the same block window as the victim's transaction.
pub const SANDWICH_WINDOW: Duration = Duration::from_secs(15);

/// A later transaction's gas price must exceed an earlier one's by at
/// least this multiple, on the same pool, to be treated as a front-run bid
/// rather than ordinary gas-price noise between unrelated traders.
pub const FRONTRUN_GAS_MULTIPLIER: f64 = 1.5;

/// Cap on how many pending swaps are tracked per pool at once. Bounds
/// worst-case memory/CPU for `observe` under a pool being hit far faster
/// than [`SANDWICH_WINDOW`] can drain it — the oldest entry is evicted
/// first, same as it would age out of the window anyway.
const MAX_TRACKED_PER_POOL: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandwichPattern {
    /// A higher-gas swap from a different address arrived on the same
    /// pool shortly after this one, bidding to execute first.
    FrontrunBid,
    /// The same address appears both before and after another address's
    /// swap on the same pool within the window — a completed bracket.
    Bracket,
}

#[derive(Debug, Clone)]
pub struct SandwichAlert {
    pub pool: PoolKey,
    pub attacker: String,
    pub victim_hash: String,
    pub pattern: SandwichPattern,
}

struct Observed {
    tx: DecodedPendingTx,
    seen_at: Instant,
}

#[derive(Default)]
pub struct SandwichDetector {
    windows: HashMap<PoolKey, Vec<Observed>>,
}

impl SandwichDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `tx` against its pool's window and returns every alert this
    /// observation raises (usually zero or one).
    pub fn observe(&mut self, tx: DecodedPendingTx, pool: PoolKey) -> Vec<SandwichAlert> {
        self.observe_at(tx, pool, Instant::now())
    }

    fn observe_at(
        &mut self,
        tx: DecodedPendingTx,
        pool: PoolKey,
        now: Instant,
    ) -> Vec<SandwichAlert> {
        let window = self.windows.entry(pool.clone()).or_default();
        window.retain(|o| now.duration_since(o.seen_at) <= SANDWICH_WINDOW);

        let mut alerts = Vec::new();

        for o in window.iter() {
            if o.tx.from == tx.from {
                continue;
            }
            if o.tx.gas_price_wei > 0
                && tx.gas_price_wei as f64 >= o.tx.gas_price_wei as f64 * FRONTRUN_GAS_MULTIPLIER
            {
                alerts.push(SandwichAlert {
                    pool: pool.clone(),
                    attacker: tx.from.clone(),
                    victim_hash: o.tx.hash.clone(),
                    pattern: SandwichPattern::FrontrunBid,
                });
            }
        }

        // Bracket: `tx.from` already appears earlier in the window, and at
        // least one *other* address's swap landed in between.
        if let Some(earlier_from_same_sender) = window.iter().find(|o| o.tx.from == tx.from) {
            if let Some(bracketed) = window
                .iter()
                .find(|o| o.tx.from != tx.from && o.seen_at >= earlier_from_same_sender.seen_at)
            {
                alerts.push(SandwichAlert {
                    pool: pool.clone(),
                    attacker: tx.from.clone(),
                    victim_hash: bracketed.tx.hash.clone(),
                    pattern: SandwichPattern::Bracket,
                });
            }
        }

        if window.len() >= MAX_TRACKED_PER_POOL {
            window.remove(0);
        }
        window.push(Observed { tx, seen_at: now });

        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::Chain;
    use crate::mempool::decoder::DecodedKind;

    fn tx(
        hash: &str,
        from: &str,
        gas_price_wei: u128,
        pool: &PoolKey,
    ) -> (DecodedPendingTx, PoolKey) {
        (
            DecodedPendingTx {
                hash: hash.to_string(),
                from: from.to_string(),
                gas_price_wei,
                kind: DecodedKind::DexSwap { pool: pool.clone() },
            },
            pool.clone(),
        )
    }

    fn eth_usdc() -> PoolKey {
        PoolKey::new(Chain::Ethereum, "ETH", "USDC")
    }

    #[test]
    fn no_alert_for_a_single_isolated_swap() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let (tx1, pool1) = tx("0x1", "0xalice", 30_000_000_000, &pool);
        assert!(detector.observe(tx1, pool1).is_empty());
    }

    #[test]
    fn higher_gas_swap_from_another_address_raises_a_frontrun_alert() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        let (victim, p) = tx("0xvictim", "0xalice", 20_000_000_000, &pool);
        detector.observe_at(victim, p, now);

        let (attacker, p) = tx("0xattack", "0xbot", 40_000_000_000, &pool); // 2x gas
        let alerts = detector.observe_at(attacker, p, now + Duration::from_secs(1));

        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].pattern, SandwichPattern::FrontrunBid);
        assert_eq!(alerts[0].attacker, "0xbot");
        assert_eq!(alerts[0].victim_hash, "0xvictim");
    }

    #[test]
    fn similar_gas_price_from_another_address_does_not_alert() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        let (first, p) = tx("0x1", "0xalice", 20_000_000_000, &pool);
        detector.observe_at(first, p, now);

        // Only 10% higher gas: ordinary noise, not a front-run bid.
        let (second, p) = tx("0x2", "0xbob", 22_000_000_000, &pool);
        let alerts = detector.observe_at(second, p, now + Duration::from_secs(1));

        assert!(alerts.is_empty());
    }

    #[test]
    fn same_address_resubmitting_does_not_alert_itself() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        let (first, p) = tx("0x1", "0xalice", 20_000_000_000, &pool);
        detector.observe_at(first, p, now);

        // Alice re-broadcasts her own tx at a much higher gas price (e.g.
        // a replacement/speedup) — not a front-run against herself.
        let (second, p) = tx("0x2", "0xalice", 60_000_000_000, &pool);
        let alerts = detector.observe_at(second, p, now + Duration::from_secs(1));

        assert!(alerts.is_empty());
    }

    #[test]
    fn bracket_pattern_detected_when_same_sender_appears_before_and_after_a_victim() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        let (front, p) = tx("0xfront", "0xbot", 20_000_000_000, &pool);
        detector.observe_at(front, p, now);

        let (victim, p) = tx("0xvictim", "0xalice", 20_000_000_000, &pool);
        detector.observe_at(victim, p, now + Duration::from_secs(1));

        let (back, p) = tx("0xback", "0xbot", 20_000_000_000, &pool);
        let alerts = detector.observe_at(back, p, now + Duration::from_secs(2));

        assert!(alerts.iter().any(|a| a.pattern == SandwichPattern::Bracket));
        let bracket = alerts
            .iter()
            .find(|a| a.pattern == SandwichPattern::Bracket)
            .unwrap();
        assert_eq!(bracket.attacker, "0xbot");
        assert_eq!(bracket.victim_hash, "0xvictim");
    }

    #[test]
    fn different_pools_are_tracked_independently() {
        let mut detector = SandwichDetector::new();
        let now = Instant::now();
        let pool_a = PoolKey::new(Chain::Ethereum, "ETH", "USDC");
        let pool_b = PoolKey::new(Chain::Ethereum, "SOL", "USDC");

        let (tx_a, p) = tx("0x1", "0xalice", 20_000_000_000, &pool_a);
        detector.observe_at(tx_a, p, now);

        // Same high-gas pattern, but on a different pool: no cross-pool alert.
        let (tx_b, p) = tx("0x2", "0xbot", 40_000_000_000, &pool_b);
        let alerts = detector.observe_at(tx_b, p, now + Duration::from_secs(1));

        assert!(alerts.is_empty());
    }

    #[test]
    fn entries_outside_the_window_are_pruned_and_do_not_alert() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        let (first, p) = tx("0x1", "0xalice", 20_000_000_000, &pool);
        detector.observe_at(first, p, now);

        // Well outside SANDWICH_WINDOW: must not trigger a front-run alert
        // against a swap that's long gone.
        let (second, p) = tx("0x2", "0xbot", 100_000_000_000, &pool);
        let alerts = detector.observe_at(second, p, now + SANDWICH_WINDOW + Duration::from_secs(5));

        assert!(alerts.is_empty());
    }

    #[test]
    fn tracked_entries_per_pool_are_bounded() {
        let mut detector = SandwichDetector::new();
        let pool = eth_usdc();
        let now = Instant::now();

        for i in 0..(MAX_TRACKED_PER_POOL + 10) {
            let (t, p) = tx(
                &format!("0x{i}"),
                &format!("0xsender{i}"),
                20_000_000_000,
                &pool,
            );
            detector.observe_at(t, p, now);
        }

        assert!(detector.windows.get(&pool).unwrap().len() <= MAX_TRACKED_PER_POOL);
    }
}
