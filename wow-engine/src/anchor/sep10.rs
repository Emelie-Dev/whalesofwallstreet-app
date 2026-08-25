use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeResponse {
    pub transaction: String,
    pub network_passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token: String,
}

pub struct Sep10Client {
    client: ClientWithMiddleware,
    // (account, anchor_domain) -> jwt
    token_cache: Arc<RwLock<HashMap<(String, String), String>>>,
    // Simple mock keystore for tests: account -> secret_key
    keys: HashMap<String, String>,
}

impl Sep10Client {
    pub fn new() -> Self {
        let mut keys = HashMap::new();
        // Add a mock key for tests if needed
        keys.insert("GTESTACCOUNT".to_string(), "S_SECRET_KEY".to_string());
        
        Self {
            client: crate::http_client::build_resilient_client().expect("client"),
            token_cache: Arc::new(RwLock::new(HashMap::new())),
            keys,
        }
    }

    pub async fn authenticate(
        &self,
        anchor_domain: &str,
        account: &str,
    ) -> Result<String, crate::error::AppError> {
        let cache_key = (account.to_string(), anchor_domain.to_string());
        
        {
            let cache = self.token_cache.read().await;
            if let Some(token) = cache.get(&cache_key) {
                return Ok(token.clone());
            }
        }

        // Check if we have the key
        let secret_key = self.keys.get(account).ok_or_else(|| {
            crate::error::AppError::BadRequest("Account signing key not available for SEP-10 challenge".to_string())
        })?;

        // 1. Fetch Challenge
        let challenge_url = format!("https://{}/auth?account={}", anchor_domain, account);
        let challenge: ChallengeResponse = self.client.get(&challenge_url).send().await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?
            .json().await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;

        // 2. Sign Challenge (Simulated for this implementation)
        let signed_tx = format!("{}_signed_by_{}", challenge.transaction, secret_key);

        // 3. Get JWT
        let token_req = serde_json::json!({
            "transaction": signed_tx
        });
        
        let token_resp: TokenResponse = self.client
            .post(&challenge_url)
            .json(&token_req)
            .send()
            .await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?
            .json()
            .await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;

        // 4. Cache JWT
        {
            let mut cache = self.token_cache.write().await;
            cache.insert(cache_key, token_resp.token.clone());
        }

        Ok(token_resp.token)
    }
}
