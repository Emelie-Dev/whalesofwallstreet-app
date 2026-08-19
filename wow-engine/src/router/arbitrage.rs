//! Bellman-Ford negative-cycle detection for cross-DEX / cross-bridge
//! arbitrage.
//!
//! [`super::RoutePlanner::find_best_route`] uses Dijkstra, which cannot
//! handle negative edge weights and so is structurally blind to arbitrage
//! loops: sequences of trades that, net of fees, return more of an asset
//! than they started with. This module models the liquidity graph in
//! log-space (`weight = -ln(exchange_rate)`) so that an arbitrage loop
//! becomes a negative-weight cycle, and uses Bellman-Ford — the classical
//! shortest-path algorithm that tolerates negative weights and can detect
//! negative cycles — to find one.

use crate::bridge::Chain;

/// Numerical slack for float-noise around zero when comparing distances.
/// Without this, floating-point rounding on a perfectly fee-conserving loop
/// (product of rates == 1.0 up to rounding) could register as a spurious
/// "negative" cycle.
const EPSILON: f64 = 1e-9;

/// Reference trade size used when snapshotting live edges (see
/// [`super::RoutePlanner::snapshot_arbitrage_edges`]): large enough to avoid
/// derivative underflow in the AMM math, small enough that its own price
/// impact is a handful of basis points rather than a real user-sized
/// trade's, so it approximates each edge's spot rate.
pub const ARBITRAGE_REFERENCE_UNITS: u64 = 100;

/// A `(chain, asset)` pair — one node in the arbitrage graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GraphNode {
    pub chain: Chain,
    pub asset: String,
}

impl GraphNode {
    pub fn new(chain: Chain, asset: impl Into<String>) -> Self {
        Self {
            chain,
            asset: asset.into(),
        }
    }
}

/// One directed conversion in the arbitrage graph: trading 1 unit of `from`
/// yields `rate` units of `to`, net of fees and price impact. `label`
/// documents which DEX/bridge leg this edge represents, so a detected cycle
/// can be reported in human-readable form.
#[derive(Debug, Clone)]
pub struct ArbitrageEdge {
    pub from: GraphNode,
    pub to: GraphNode,
    pub rate: f64,
    pub label: String,
}

/// A confirmed arbitrage loop: executing every edge in `cycle_edges` in
/// order, starting from `cycle[0]`, returns more of the starting asset than
/// you began with.
#[derive(Debug, Clone)]
pub struct ArbitrageDetected {
    /// Nodes visited, in order. First and last entries are the same node.
    pub cycle: Vec<GraphNode>,
    /// Human-readable label for each edge traversed, in order.
    pub cycle_edges: Vec<String>,
    /// Net multiplier of executing the full cycle once (e.g. `1.012` means
    /// a 1.2% profit per loop, before gas/slippage on the real trade).
    pub profit_multiplier: f64,
}

/// Runs Bellman-Ford over `edges` to find a negative-weight cycle in
/// log-space, i.e. an arbitrage loop. Returns the first one found, or
/// `None` if the graph has none.
///
/// Iteration count is bounded at `num_nodes + 1` relaxation passes — the
/// standard, provably-sufficient bound for Bellman-Ford (`|V| - 1` passes
/// guarantee correct shortest-path distances when there's no negative
/// cycle; one more pass is the negative-cycle detector). Because the outer
/// loop is a fixed `for` range rather than a "relax until nothing changes"
/// `while`, this always terminates in `O(|V| * |E|)` regardless of graph
/// shape — it cannot loop indefinitely even on a pathological or malformed
/// input.
pub fn detect_arbitrage(edges: &[ArbitrageEdge]) -> Option<ArbitrageDetected> {
    if edges.is_empty() {
        return None;
    }

    // Assign each distinct node a dense index for array-backed distance /
    // predecessor tables.
    let mut node_index: std::collections::HashMap<GraphNode, usize> =
        std::collections::HashMap::new();
    for edge in edges {
        let next = node_index.len();
        node_index.entry(edge.from.clone()).or_insert(next);
        let next = node_index.len();
        node_index.entry(edge.to.clone()).or_insert(next);
    }
    let num_nodes = node_index.len();
    // Every slot below is overwritten: `node_index` maps each node to a
    // unique index in `0..num_nodes`, so this placeholder value never
    // survives the loop.
    let mut nodes: Vec<GraphNode> = vec![GraphNode::new(Chain::Ethereum, ""); num_nodes];
    for (node, &idx) in &node_index {
        nodes[idx] = node.clone();
    }

    // Log-space weights: a negative cycle here is an arbitrage loop
    // (product of rates > 1, so sum of -ln(rate) < 0). Non-positive rates
    // aren't valid exchange rates and are dropped rather than producing a
    // NaN/infinite weight.
    let weighted_edges: Vec<(usize, usize, f64, f64, &str)> = edges
        .iter()
        .filter(|e| e.rate > 0.0)
        .filter_map(|e| {
            let u = *node_index.get(&e.from)?;
            let v = *node_index.get(&e.to)?;
            Some((u, v, -e.rate.ln(), e.rate, e.label.as_str()))
        })
        .collect();

    // Virtual source at distance 0 to every node (implicit: `dist` starts
    // all-zero instead of all-infinity), so a negative cycle is found
    // anywhere in the graph rather than only ones reachable from one
    // arbitrarily chosen start node.
    let mut dist = vec![0.0_f64; num_nodes];
    let mut pred_edge: Vec<Option<usize>> = vec![None; num_nodes];
    let mut relaxed_on_final_pass: Option<usize> = None;

    for pass in 0..=num_nodes {
        let mut relaxed_any = false;
        for (ei, &(u, v, w, _, _)) in weighted_edges.iter().enumerate() {
            if dist[u] + w < dist[v] - EPSILON {
                dist[v] = dist[u] + w;
                pred_edge[v] = Some(ei);
                relaxed_any = true;
                if pass == num_nodes {
                    relaxed_on_final_pass = Some(v);
                }
            }
        }
        if !relaxed_any {
            break;
        }
    }

    let start = relaxed_on_final_pass?;

    // Walk predecessors `num_nodes` steps back from a node that was still
    // relaxing on the final pass: this is guaranteed to land strictly
    // inside the negative cycle (not just somewhere upstream of it).
    let mut on_cycle = start;
    for _ in 0..num_nodes {
        on_cycle = weighted_edges[pred_edge[on_cycle]?].0;
    }

    // Trace the cycle itself by following predecessor edges until we
    // return to `on_cycle`.
    let mut cycle_edge_idxs = Vec::new();
    let mut current = on_cycle;
    loop {
        let ei = pred_edge[current]?;
        cycle_edge_idxs.push(ei);
        current = weighted_edges[ei].0;
        if current == on_cycle {
            break;
        }
    }
    cycle_edge_idxs.reverse();

    let mut cycle = vec![nodes[on_cycle].clone()];
    let mut cycle_edges = Vec::with_capacity(cycle_edge_idxs.len());
    let mut profit_multiplier = 1.0_f64;
    for ei in cycle_edge_idxs {
        let (_, v, _, rate, label) = weighted_edges[ei];
        cycle.push(nodes[v].clone());
        cycle_edges.push(label.to_string());
        profit_multiplier *= rate;
    }

    Some(ArbitrageDetected {
        cycle,
        cycle_edges,
        profit_multiplier,
    })
}

/// Interval between arbitrage-scan passes.
pub const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs forever: every [`SCAN_INTERVAL`], takes a fresh liquidity snapshot
/// via `planner` (see [`super::RoutePlanner::scan_for_arbitrage`]) and
/// checks it for an arbitrage loop.
///
/// This holds no state or locks in common with the request-serving
/// `find_best_route` path — it only reads through the same read-only mock
/// providers — so a slow or stalled scan pass can never add latency to a
/// live `/quote` request. A detected loop is logged as a warning; this
/// codebase has no separate event bus, so `tracing` (already the
/// observability stack every `#[tracing::instrument]` call feeds) is where
/// it's surfaced.
pub async fn run_arbitrage_scanner(planner: std::sync::Arc<super::RoutePlanner>) {
    loop {
        match planner.scan_for_arbitrage().await {
            Some(found) => {
                tracing::warn!(
                    profit_multiplier = found.profit_multiplier,
                    cycle = ?found.cycle,
                    edges = ?found.cycle_edges,
                    "ArbitrageDetected: negative-weight cycle found in liquidity graph"
                );
            }
            None => {
                tracing::debug!("arbitrage scan: no negative cycle found");
            }
        }
        tokio::time::sleep(SCAN_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(asset: &str) -> GraphNode {
        // Every synthetic test graph lives on one chain; only the asset
        // axis matters for these tests, so Ethereum is an arbitrary choice.
        GraphNode::new(Chain::Ethereum, asset)
    }

    #[test]
    fn test_empty_edges_returns_none() {
        assert!(detect_arbitrage(&[]).is_none());
    }

    #[test]
    fn test_detects_simple_two_node_arbitrage_cycle() {
        // A -> B at 2.0, B -> A at 0.6: round trip multiplies by 1.2, a 20%
        // profit loop hidden among perfectly ordinary-looking rates.
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 2.0,
                label: "A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("A"),
                rate: 0.6,
                label: "B->A".to_string(),
            },
        ];

        let found = detect_arbitrage(&edges).expect("should detect the arbitrage loop");
        assert!(
            (found.profit_multiplier - 1.2).abs() < 1e-6,
            "expected ~1.2x profit multiplier, got {}",
            found.profit_multiplier
        );
        assert_eq!(found.cycle.first(), found.cycle.last());
        assert_eq!(found.cycle_edges.len(), 2);
    }

    #[test]
    fn test_detects_three_node_hidden_cycle_among_noise_edges() {
        // A -> B -> C -> A each at 1.05: round trip is 1.05^3 ≈ 1.157, an
        // ~15.8% profit loop. Surrounded by unrelated "noise" edges (D, E)
        // that touch none of the cycle's nodes, so detection has to find
        // the real cycle rather than just reporting "some edge exists".
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 1.05,
                label: "A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("C"),
                rate: 1.05,
                label: "B->C".to_string(),
            },
            ArbitrageEdge {
                from: node("C"),
                to: node("A"),
                rate: 1.05,
                label: "C->A".to_string(),
            },
            // Noise: a lossy, non-cyclic chain D -> E -> D that never
            // returns to profit and shares no node with A/B/C.
            ArbitrageEdge {
                from: node("D"),
                to: node("E"),
                rate: 0.9,
                label: "D->E".to_string(),
            },
            ArbitrageEdge {
                from: node("E"),
                to: node("D"),
                rate: 0.9,
                label: "E->D".to_string(),
            },
        ];

        let found = detect_arbitrage(&edges).expect("should detect the 3-node arbitrage loop");
        assert!(
            (found.profit_multiplier - 1.05_f64.powi(3)).abs() < 1e-6,
            "expected ~{}x profit multiplier, got {}",
            1.05_f64.powi(3),
            found.profit_multiplier
        );
        // Every node in the reported cycle must be one of A/B/C, not the
        // unrelated noise nodes.
        for n in &found.cycle {
            assert!(
                ["A", "B", "C"].contains(&n.asset.as_str()),
                "cycle should only contain A/B/C, found {:?}",
                n
            );
        }
    }

    #[test]
    fn test_no_false_positive_on_value_conserving_loop() {
        // A -> B -> A at exactly reciprocal rates: round trip is exactly
        // 1.0, break-even, not an arbitrage. Must not be reported.
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 3.0,
                label: "A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("A"),
                rate: 1.0 / 3.0,
                label: "B->A".to_string(),
            },
        ];

        assert!(detect_arbitrage(&edges).is_none());
    }

    #[test]
    fn test_no_false_positive_when_every_leg_loses_value() {
        // Every leg of a closed loop charges a fee (rate < the fair
        // reciprocal), the realistic case for a fee-bearing DEX/bridge
        // graph: no profit is ever possible, so nothing should be flagged.
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 0.99,
                label: "A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("C"),
                rate: 0.99,
                label: "B->C".to_string(),
            },
            ArbitrageEdge {
                from: node("C"),
                to: node("A"),
                rate: 0.99,
                label: "C->A".to_string(),
            },
        ];

        assert!(detect_arbitrage(&edges).is_none());
    }

    #[test]
    fn test_non_positive_rate_is_ignored_not_a_panic() {
        // A malformed/zero rate must be dropped, not produce a NaN/infinite
        // log-space weight that corrupts the rest of the graph.
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 0.0,
                label: "A->B (bad)".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("A"),
                rate: 1.5,
                label: "B->A".to_string(),
            },
        ];

        assert!(detect_arbitrage(&edges).is_none());
    }

    #[test]
    fn test_terminates_on_large_acyclic_graph() {
        // A long chain of 500 nodes with no cycle at all: this only
        // matters as a bound-sanity check, since the outer loop is a fixed
        // `0..=num_nodes` range regardless of graph shape, but it confirms
        // that holds in practice on a graph much larger than the engine's
        // real (tiny, ~16-node) liquidity graph.
        let mut edges = Vec::new();
        for i in 0..500 {
            edges.push(ArbitrageEdge {
                from: node(&format!("n{i}")),
                to: node(&format!("n{}", i + 1)),
                rate: 0.999,
                label: format!("n{i}->n{}", i + 1),
            });
        }

        assert!(detect_arbitrage(&edges).is_none());
    }

    #[test]
    fn test_multigraph_uses_the_edge_that_forms_the_cycle() {
        // Two parallel A->B edges (different "providers"); only combined
        // with the better one does the B->A leg form a profitable loop.
        // The reconstructed cycle must reference the provider that's
        // actually responsible, not just the first A->B edge in the list.
        let edges = vec![
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 0.5, // ProviderX: too lossy to ever profit
                label: "ProviderX A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("A"),
                to: node("B"),
                rate: 2.0, // ProviderY: this is the one that makes the loop profitable
                label: "ProviderY A->B".to_string(),
            },
            ArbitrageEdge {
                from: node("B"),
                to: node("A"),
                rate: 0.6,
                label: "B->A".to_string(),
            },
        ];

        let found = detect_arbitrage(&edges).expect("should detect the loop via ProviderY");
        assert!(found.cycle_edges.iter().any(|e| e.contains("ProviderY")));
        assert!(!found.cycle_edges.iter().any(|e| e.contains("ProviderX")));
    }
}
