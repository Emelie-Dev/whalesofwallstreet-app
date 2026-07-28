use crate::anchor::Sep24InteractiveResponse;
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

pub struct Sep24Client {
    #[allow(dead_code)]
    client: ClientWithMiddleware,
    tracker: Arc<super::tracker::TrackerStore>,
}

impl Sep24Client {
    pub fn new(tracker: Arc<super::tracker::TrackerStore>) -> Self {
        Self {
            client: crate::http_client::build_resilient_client()
                .expect("Failed to build resilient HTTP client"),
            tracker,
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

        let interactive_url = format!(
            "https://{}/sep24/interactive/{}?asset_code={}&account={}&transaction_id={}&callback=postMessage",
            anchor_domain, kind, asset_code, account, tx_id
        );

        Ok(Sep24InteractiveResponse {
            r#type: "interactive_customer_info_needed".to_string(),
            url: interactive_url,
            id: tx_id,
        })
    }
}
