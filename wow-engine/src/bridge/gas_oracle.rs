use crate::bridge::Chain;
use moka::future::Cache;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::{info, warn};

pub struct GasOracle {
    cache: Cache<Chain, f64>,
    client: Client,
}

impl GasOracle {
    pub fn new() -> Self {
        // Cache with 60 seconds TTL
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60))
            .build();

        // Fix #2: Enforce a strict HTTP timeout of 3 seconds
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .expect("Failed to build HTTP client for GasOracle");

        Self { cache, client }
    }

    pub async fn estimate_gas_fee_usd(&self, chain: Chain) -> f64 {
        // Fix #1: Use try_get_with to coalesce concurrent calls and fix Cache Stampede
        let fee = self
            .cache
            .try_get_with(chain, async { self.fetch_from_api(chain).await })
            .await;

        match fee {
            Ok(val) => val,
            Err(e) => {
                warn!(
                    "Failed to fetch gas fee for {:?} from oracle: {}. Using fallback.",
                    chain, e
                );
                Self::fallback_fee(chain)
            }
        }
    }

    /// Evicts the cached gas fee for a single chain.
    ///
    /// Used both by local logic and by [`crate::cache_sync`] when a
    /// cluster-wide invalidation message arrives for this chain, so that the
    /// next call to [`estimate_gas_fee_usd`](Self::estimate_gas_fee_usd)
    /// re-fetches instead of serving a stale value for the rest of the TTL.
    pub async fn invalidate(&self, chain: Chain) {
        self.cache.invalidate(&chain).await;
    }

    /// Evicts every cached gas fee, regardless of chain.
    pub async fn invalidate_all(&self) {
        self.cache.invalidate_all();
    }

    async fn fetch_from_api(&self, chain: Chain) -> Result<f64, std::sync::Arc<anyhow::Error>> {
        info!(
            "Fetching real-time gas fee from external REST API for {:?}",
            chain
        );
        // Fix #3: Actual REST integrations for EVM chains
        let result = match chain {
            Chain::Ethereum => self.fetch_etherscan().await,
            Chain::Arbitrum => self.fetch_arbiscan().await,
            // For Solana/Stellar, there are no simple unauthenticated gas APIs, returning a safe fallback
            _ => Ok(Self::fallback_fee(chain)),
        };

        result.map_err(std::sync::Arc::new)
    }

    async fn fetch_etherscan(&self) -> Result<f64, anyhow::Error> {
        let url = "https://api.etherscan.io/api?module=gastracker&action=gasoracle";
        let resp: Value = self.client.get(url).send().await?.json().await?;

        // Etherscan returns "ProposeGasPrice" in Gwei
        if let Some(price_str) = resp
            .get("result")
            .and_then(|r| r.get("ProposeGasPrice"))
            .and_then(|p| p.as_str())
        {
            let gwei: f64 = price_str.parse()?;
            // Assume 150,000 gas limit for a bridge tx, and $3000 per ETH
            // Fee (USD) = gas_limit * gas_price_gwei * 10^-9 * eth_price_usd
            let fee_usd = 150_000.0 * gwei * 1e-9 * 3000.0;
            return Ok(fee_usd);
        }
        Err(anyhow::anyhow!("Invalid Etherscan response"))
    }

    async fn fetch_arbiscan(&self) -> Result<f64, anyhow::Error> {
        let url = "https://api.arbiscan.io/api?module=gastracker&action=gasoracle";
        let resp: Value = self.client.get(url).send().await?.json().await?;

        if let Some(price_str) = resp
            .get("result")
            .and_then(|r| r.get("ProposeGasPrice"))
            .and_then(|p| p.as_str())
        {
            let gwei: f64 = price_str.parse()?;
            let fee_usd = 1_000_000.0 * gwei * 1e-9 * 3000.0; // L2 gas limit is higher, but gwei is tiny
            return Ok(fee_usd);
        }
        Err(anyhow::anyhow!("Invalid Arbiscan response"))
    }

    fn fallback_fee(chain: Chain) -> f64 {
        match chain {
            Chain::Ethereum => 15.00,
            Chain::Arbitrum => 0.50,
            Chain::Solana => 0.05,
            Chain::Stellar => 0.01,
        }
    }
}

impl Default for GasOracle {
    fn default() -> Self {
        Self::new()
    }
}

/// Test-only direct cache accessors, used by [`crate::cache_sync`]'s tests to
/// seed/observe cache state without going through a real (network-bound) fetch.
#[cfg(test)]
impl GasOracle {
    pub(crate) async fn cache_insert_for_test(&self, chain: Chain, fee: f64) {
        self.cache.insert(chain, fee).await;
    }

    pub(crate) async fn cached_value_for_test(&self, chain: Chain) -> Option<f64> {
        self.cache.get(&chain).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalidate_evicts_only_the_targeted_chain() {
        let oracle = GasOracle::new();
        oracle.cache.insert(Chain::Ethereum, 42.0).await;
        oracle.cache.insert(Chain::Solana, 1.0).await;

        oracle.invalidate(Chain::Ethereum).await;

        assert_eq!(oracle.cache.get(&Chain::Ethereum).await, None);
        assert_eq!(oracle.cache.get(&Chain::Solana).await, Some(1.0));
    }

    #[tokio::test]
    async fn invalidate_all_evicts_every_chain() {
        let oracle = GasOracle::new();
        oracle.cache.insert(Chain::Ethereum, 42.0).await;
        oracle.cache.insert(Chain::Solana, 1.0).await;

        oracle.invalidate_all().await;

        assert_eq!(oracle.cache.get(&Chain::Ethereum).await, None);
        assert_eq!(oracle.cache.get(&Chain::Solana).await, None);
    }
}
