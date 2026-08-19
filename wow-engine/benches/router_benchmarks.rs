use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::Arc;
use tokio::runtime::Runtime;
use wow_engine::bridge::Chain;
use wow_engine::config::AppConfig;
use wow_engine::router::arbitrage::{detect_arbitrage, ArbitrageEdge, GraphNode};
use wow_engine::router::RoutePlanner;

fn bench_router(c: &mut Criterion) {
    // Set mock gas oracle environment variable to bypass external HTTP calls
    std::env::set_var("MOCK_GAS_ORACLE", "true");

    let rt = Runtime::new().unwrap();
    let planner = RoutePlanner::new(Arc::new(AppConfig::default()));

    let mut group = c.benchmark_group("routing_engine");
    group.sample_size(100);

    // 1. Solana -> Stellar USDC (USDC-to-USDC) - Single-path Dijkstra Routing
    group.bench_function("dijkstra_single_path_usdc", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = planner
                .find_best_route(
                    Chain::Solana,
                    Chain::Stellar,
                    "USDC",
                    "USDC",
                    10000,
                    false, // multi_path = false (single-path Dijkstra)
                )
                .await;
        });
    });

    // 2. Solana -> Stellar USDC (USDC-to-USDC) - Multi-path / Max-flow Routing
    group.bench_function("multipath_search_usdc", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = planner
                .find_best_route(
                    Chain::Solana,
                    Chain::Stellar,
                    "USDC",
                    "USDC",
                    10000000, // high liquidity/amount
                    true,     // multi_path = true
                )
                .await;
        });
    });

    // 3. Ethereum -> Stellar XLM (Multi-hop cross-chain / multi-asset) - Single-path Dijkstra Routing
    group.bench_function("dijkstra_single_path_multi_hop", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = planner
                .find_best_route(
                    Chain::Ethereum,
                    Chain::Stellar,
                    "ETH",
                    "XLM",
                    1,
                    false, // multi_path = false
                )
                .await;
        });
    });

    // 4. Ethereum -> Stellar XLM (Multi-hop cross-chain / multi-asset) - Multi-path / Max-flow Routing
    group.bench_function("multipath_search_multi_hop", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = planner
                .find_best_route(
                    Chain::Ethereum,
                    Chain::Stellar,
                    "ETH",
                    "XLM",
                    1,
                    true, // multi_path = true
                )
                .await;
        });
    });

    group.finish();

    // Bellman-Ford arbitrage detection (Issue #33): benchmarked separately
    // from routing above so a regression in one is never masked by, or
    // mistaken for, a regression in the other.
    let mut arb_group = c.benchmark_group("arbitrage_detection");
    arb_group.sample_size(100);

    // 1. Pure Bellman-Ford pass over a synthetic graph sized like the
    // engine's real liquidity graph (4 chains x 4 assets = 16 DEX nodes,
    // fully connected, plus a hidden negative cycle) - isolates the
    // algorithm's own cost from any provider/network latency.
    let synthetic_edges = build_synthetic_graph_with_hidden_cycle();
    arb_group.bench_function("bellman_ford_synthetic_16_node_graph", |b| {
        b.iter(|| detect_arbitrage(&synthetic_edges));
    });

    // 2. End-to-end live scan: snapshotting the real liquidity graph
    // through every DEX/bridge provider, then running detection over it.
    // This is what `run_arbitrage_scanner` actually pays on each pass, and
    // confirms it stays cheap enough to never compete with request-serving
    // routing for resources.
    arb_group.bench_function("live_scan_for_arbitrage", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = planner.scan_for_arbitrage().await;
        });
    });

    arb_group.finish();

    // `RoutePlanner` holds a `DeBridgeClient` whose `Drop` impl calls
    // `tokio::spawn`, which panics outside a runtime context. Drop it here,
    // inside the runtime, instead of letting it fall out of scope after
    // `rt` when the function returns.
    rt.block_on(async move {
        drop(planner);
    });
}

/// A synthetic 16-node graph (matching the engine's 4 chains x 4 assets
/// liquidity graph in size) with one hidden 3-node negative cycle buried
/// among lossy, non-cyclic "noise" edges — representative of the worst case
/// `detect_arbitrage` has to fully search rather than short-circuit on.
fn build_synthetic_graph_with_hidden_cycle() -> Vec<ArbitrageEdge> {
    let chains = [
        Chain::Ethereum,
        Chain::Arbitrum,
        Chain::Solana,
        Chain::Stellar,
    ];
    let assets = ["ETH", "USDC", "SOL", "XLM"];

    let mut edges = Vec::new();
    for &chain in &chains {
        for &from in &assets {
            for &to in &assets {
                if from == to {
                    continue;
                }
                edges.push(ArbitrageEdge {
                    from: GraphNode::new(chain, from),
                    to: GraphNode::new(chain, to),
                    rate: 0.98, // every "real" leg loses a bit of value
                    label: format!("noise {chain:?} {from}->{to}"),
                });
            }
        }
    }

    // Bury a profitable 3-cycle among the 180 lossy noise edges above.
    edges.push(ArbitrageEdge {
        from: GraphNode::new(Chain::Ethereum, "ETH"),
        to: GraphNode::new(Chain::Arbitrum, "ETH"),
        rate: 1.05,
        label: "hidden leg 1".to_string(),
    });
    edges.push(ArbitrageEdge {
        from: GraphNode::new(Chain::Arbitrum, "ETH"),
        to: GraphNode::new(Chain::Solana, "ETH"),
        rate: 1.05,
        label: "hidden leg 2".to_string(),
    });
    edges.push(ArbitrageEdge {
        from: GraphNode::new(Chain::Solana, "ETH"),
        to: GraphNode::new(Chain::Ethereum, "ETH"),
        rate: 1.05,
        label: "hidden leg 3".to_string(),
    });

    edges
}

criterion_group!(benches, bench_router);
criterion_main!(benches);
