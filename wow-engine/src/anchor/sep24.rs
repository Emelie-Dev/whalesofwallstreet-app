use crate::anchor::Sep24InteractiveResponse;
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

pub struct Sep24Client {
    #[allow(dead_code)]
    client: ClientWithMiddleware,
    tracker: Arc<super::tracker::TrackerStore>,
    sep10: super::sep10::Sep10Client,
}

impl Sep24Client {
    pub fn new(
        tracker: Arc<super::tracker::TrackerStore>,
        sep10_signing_keys: &[String],
        challenge_max_age_secs: i64,
        challenge_max_future_skew_secs: i64,
    ) -> Result<Self, anyhow::Error> {
        let sep10 = super::sep10::Sep10Client::from_config(
            sep10_signing_keys,
            challenge_max_age_secs,
            challenge_max_future_skew_secs,
        )?;
        Ok(Self {
            client: crate::http_client::build_resilient_client()
                .expect("Failed to build resilient HTTP client"),
            tracker,
            sep10,
        })
    }

    pub fn new_for_tests(tracker: Arc<super::tracker::TrackerStore>) -> Self {
        Self {
            client: crate::http_client::build_resilient_client()
                .expect("Failed to build resilient HTTP client"),
            tracker,
            sep10: super::sep10::Sep10Client::new(),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn initiate_deposit(
        &self,
        anchor_domain: &str,
        asset_code: &str,
        account: &str,
    ) -> Result<Sep24InteractiveResponse, anyhow::Error> {
        self.initiate_flow("deposit", anchor_domain, asset_code, account)
            .await
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn initiate_withdrawal(
        &self,
        anchor_domain: &str,
        asset_code: &str,
        account: &str,
    ) -> Result<Sep24InteractiveResponse, anyhow::Error> {
        self.initiate_flow("withdraw", anchor_domain, asset_code, account)
            .await
    }

    /// Shared helper for both deposit and withdrawal interactive flows.
    ///
    /// Generates a transaction ID, stores the pending transaction in the global
    /// tracker, and constructs the SEP-24 interactive redirect URL for the client.
    #[tracing::instrument(skip(self), err)]
    async fn initiate_flow(
        &self,
        kind: &str,
        anchor_domain: &str,
        asset_code: &str,
        account: &str,
    ) -> Result<Sep24InteractiveResponse, anyhow::Error> {
        let tx_id = format!("tx_sep24_{}", super::generate_uuid());

        // 1. Authenticate via SEP-10
        let jwt = self.sep10.authenticate(anchor_domain, account).await?;

        // 2. Insert into tracker
        self.tracker
            .insert_transaction(super::tracker::Transaction {
                id: tx_id.clone(),
                status: "pending_user_transfer_start".to_string(),
                asset_code: asset_code.to_string(),
                account: account.to_string(),
                amount_in: None,
                amount_out: None,
            })
            .await?;

        // 3. Make POST request to interactive endpoint
        let endpoint = format!(
            "https://{}/sep24/transactions/{}/interactive",
            anchor_domain, kind
        );
        let resp = self
            .client
            .post(&endpoint)
            .bearer_auth(jwt)
            .json(&serde_json::json!({
                "asset_code": asset_code,
                "account": account,
            }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Anchor rejected interactive flow: {} - {}",
                status,
                body
            ));
        }

        let parsed: serde_json::Value = resp.json().await?;
        let interactive_url = parsed["url"].as_str().unwrap_or("").to_string();
        let anchor_tx_id = parsed["id"].as_str().unwrap_or(&tx_id).to_string();

        Ok(Sep24InteractiveResponse {
            r#type: "interactive_customer_info_needed".to_string(),
            url: interactive_url,
            id: anchor_tx_id,
        })
    }
}

/// Builds the SEP-24 interactive redirect URL for a deposit/withdraw flow.
///
/// Split out from [`Sep24Client::initiate_flow`] so the URL format can be
/// unit tested without needing a live [`super::tracker::TrackerStore`].
#[allow(dead_code)]
fn build_interactive_url(
    kind: &str,
    anchor_domain: &str,
    asset_code: &str,
    account: &str,
    tx_id: &str,
) -> String {
    format!(
        "https://{}/sep24/interactive/{}?asset_code={}&account={}&transaction_id={}&callback=postMessage",
        anchor_domain, kind, asset_code, account, tx_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_interactive_url_deposit_is_well_formed() {
        let url = build_interactive_url(
            "deposit",
            "anchor.example.com",
            "USDC",
            "GA5Z3IX5VQ3N6FB77T342A27RWRN7CKEZ63M3W7S5VJB3D77J6F2JAFK",
            "tx_sep24_abc123",
        );

        assert_eq!(
            url,
            "https://anchor.example.com/sep24/interactive/deposit?asset_code=USDC&account=GA5Z3IX5VQ3N6FB77T342A27RWRN7CKEZ63M3W7S5VJB3D77J6F2JAFK&transaction_id=tx_sep24_abc123&callback=postMessage"
        );
    }

    #[test]
    fn test_build_interactive_url_withdraw_uses_withdraw_path() {
        let url = build_interactive_url(
            "withdraw",
            "anchor.example.com",
            "XLM",
            "GA5Z3IX5VQ3N6FB77T342A27RWRN7CKEZ63M3W7S5VJB3D77J6F2JAFK",
            "tx_sep24_xyz789",
        );

        assert!(url.contains("/sep24/interactive/withdraw"));
        assert!(url.contains("asset_code=XLM"));
    }

    /// Full round trip through `initiate_flow`, asserting the interactive
    /// response's ID matches a transaction actually persisted in the
    /// tracker. Requires a live Postgres instance (see
    /// `tests/transaction_atomicity_tests.rs` for the same convention);
    /// skipped by default since CI's `cargo test` doesn't pass
    /// `--include-ignored`.
    #[tokio::test]
    #[ignore]
    async fn test_initiate_deposit_inserts_matching_transaction_into_tracker() {
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@localhost/wow_engine_test".to_string()
        });

        let db = match crate::db::Database::new(&database_url).await {
            Ok(db) => db,
            Err(e) => {
                eprintln!("Skipping test: {}", e);
                return;
            }
        };
        db.run_migrations().await.ok();

        let tracker = std::sync::Arc::new(super::super::tracker::TrackerStore::new(db));
        let client = Sep24Client::new_for_tests(tracker.clone());

        let response = client
            .initiate_deposit("anchor.example.com", "USDC", "GTESTACCOUNT")
            .await
            .unwrap();

        assert_eq!(response.r#type, "interactive_customer_info_needed");
        assert!(response.url.contains("/sep24/interactive/deposit"));

        let stored = tracker
            .get_transaction(&response.id)
            .await
            .unwrap()
            .expect("transaction should have been inserted into the tracker");

        assert_eq!(stored.id, response.id);
        assert_eq!(stored.asset_code, "USDC");
        assert_eq!(stored.account, "GTESTACCOUNT");
        assert_eq!(stored.status, "pending_user_transfer_start");
    }
}
