use crate::anchor::Sep38Quote;
use reqwest_middleware::ClientWithMiddleware;

pub struct Sep38Client {
    #[allow(dead_code)]
    client: ClientWithMiddleware,
}

impl Default for Sep38Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Sep38Client {
    pub fn new() -> Self {
        Self {
            client: crate::http_client::build_resilient_client()
                .expect("Failed to build resilient HTTP client"),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_indicative_quote(
        &self,
        _anchor_domain: &str,
        sell_asset: &str,
        buy_asset: &str,
        sell_amount: f64,
    ) -> Result<Sep38Quote, anyhow::Error> {
        self.generate_quote(_anchor_domain, sell_asset, buy_asset, sell_amount, 15)
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_firm_quote(
        &self,
        _anchor_domain: &str,
        sell_asset: &str,
        buy_asset: &str,
        sell_amount: f64,
    ) -> Result<Sep38Quote, anyhow::Error> {
        self.generate_quote(_anchor_domain, sell_asset, buy_asset, sell_amount, 5)
        // Firm quotes expire faster
    }

    #[tracing::instrument(skip(self), err)]
    fn generate_quote(
        &self,
        _anchor_domain: &str,
        sell_asset: &str,
        buy_asset: &str,
        sell_amount: f64,
        expiration_minutes: i64,
    ) -> Result<Sep38Quote, anyhow::Error> {
        let quote_id = format!("q_sep38_{}", super::generate_uuid());

        let (price, buy_amount) = match buy_asset {
            b if b.contains("NGN") => (1450.0, sell_amount * 1450.0),
            b if b.contains("EUR") => (0.92, sell_amount * 0.92),
            _ => (1.0, sell_amount),
        };

        let expires_at = (chrono::Utc::now() + chrono::Duration::minutes(expiration_minutes))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        Ok(Sep38Quote {
            id: quote_id,
            expires_at,
            price: format!("{:.7}", price),
            sell_asset: sell_asset.to_string(),
            sell_amount: format!("{:.7}", sell_amount),
            buy_asset: buy_asset.to_string(),
            buy_amount: format!("{:.7}", buy_amount),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_quote_ngn_branch() {
        let client = Sep38Client::new();
        let quote = client
            .generate_quote("test.com", "USDC", "NGN", 100.0, 15)
            .unwrap();

        assert_eq!(quote.price, "1450.0000000");
        assert_eq!(quote.buy_amount, "145000.0000000");
        assert_eq!(quote.sell_asset, "USDC");
        assert_eq!(quote.buy_asset, "NGN");
        assert!(quote.id.starts_with("q_sep38_"));
    }

    #[test]
    fn test_generate_quote_eur_branch() {
        let client = Sep38Client::new();
        let quote = client
            .generate_quote("test.com", "USDC", "EURT", 100.0, 15)
            .unwrap();

        assert_eq!(quote.price, "0.9200000");
        assert_eq!(quote.buy_amount, "92.0000000");
    }

    #[test]
    fn test_generate_quote_default_branch_is_one_to_one() {
        let client = Sep38Client::new();
        let quote = client
            .generate_quote("test.com", "USDC", "USDC", 100.0, 15)
            .unwrap();

        assert_eq!(quote.price, "1.0000000");
        assert_eq!(quote.buy_amount, "100.0000000");
        assert_eq!(quote.sell_amount, "100.0000000");
    }

    #[tokio::test]
    async fn test_get_indicative_quote_uses_longer_expiry_than_firm_quote() {
        let client = Sep38Client::new();

        let indicative = client
            .get_indicative_quote("test.com", "USDC", "NGN", 50.0)
            .await
            .unwrap();
        let firm = client
            .get_firm_quote("test.com", "USDC", "NGN", 50.0)
            .await
            .unwrap();

        let indicative_expiry = chrono::DateTime::parse_from_rfc3339(&indicative.expires_at)
            .unwrap()
            .timestamp();
        let firm_expiry = chrono::DateTime::parse_from_rfc3339(&firm.expires_at)
            .unwrap()
            .timestamp();

        // Indicative quotes (15 min) must expire later than firm quotes (5 min).
        assert!(indicative_expiry > firm_expiry);
    }
}
