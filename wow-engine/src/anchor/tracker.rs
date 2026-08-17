use moka::future::Cache;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub status: String,
    pub asset_code: String,
    pub account: String,
    pub amount_in: Option<String>,
    pub amount_out: Option<String>,
}

pub struct TrackerStore {
    db: crate::db::Database,
    cache: Cache<String, Transaction>,
}

impl TrackerStore {
    pub fn new(db: crate::db::Database) -> Self {
        let cache = Cache::builder()
            .time_to_idle(Duration::from_secs(60 * 60)) // 1 hour TTL
            .build();
        Self { db, cache }
    }

    pub async fn insert_transaction(&self, tx: Transaction) -> Result<(), sqlx::Error> {
        // We use query! since we want to enforce schema correctness (or query_as!)
        // Wait, query! checks against the actual DB in compile time if we use sqlx offline mode.
        // Let's just use query_as without macro or query to avoid compilation issues in case offline files aren't updated.
        // Actually sqlx::query behaves fine, let's just use `sqlx::query` to avoid `prepare` requirements.

        sqlx::query(
            r#"
            INSERT INTO sep24_transactions (id, status, asset_code, account, amount_in, amount_out, updated_at) 
            VALUES ($1, $2, $3, $4, $5, $6, NOW()) 
            ON CONFLICT (id) DO UPDATE SET 
                status = EXCLUDED.status, 
                asset_code = EXCLUDED.asset_code, 
                account = EXCLUDED.account, 
                amount_in = EXCLUDED.amount_in, 
                amount_out = EXCLUDED.amount_out, 
                updated_at = NOW()
            "#
        )
        .bind(&tx.id)
        .bind(&tx.status)
        .bind(&tx.asset_code)
        .bind(&tx.account)
        .bind(&tx.amount_in)
        .bind(&tx.amount_out)
        .execute(self.db.pool())
        .await?;

        self.cache.insert(tx.id.clone(), tx).await;
        Ok(())
    }

    pub async fn get_transaction(&self, id: &str) -> Result<Option<Transaction>, sqlx::Error> {
        if let Some(tx) = self.cache.get(id).await {
            return Ok(Some(tx));
        }

        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            SELECT id, status, asset_code, account, amount_in, amount_out 
            FROM sep24_transactions 
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.db.pool())
        .await?;

        if let Some((id, status, asset_code, account, amount_in, amount_out)) = row {
            let tx = Transaction {
                id,
                status,
                asset_code,
                account,
                amount_in,
                amount_out,
            };
            self.cache.insert(tx.id.clone(), tx.clone()).await;
            Ok(Some(tx))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_store() -> Option<TrackerStore> {
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@localhost/wow_engine_test".to_string()
        });

        match crate::db::Database::new(&database_url).await {
            Ok(db) => {
                db.run_migrations().await.ok();
                Some(TrackerStore::new(db))
            }
            Err(e) => {
                eprintln!("Skipping test: {}", e);
                None
            }
        }
    }

    /// Requires a live Postgres instance; skipped by default since CI's
    /// `cargo test` doesn't pass `--include-ignored` (see
    /// `tests/transaction_atomicity_tests.rs` for the same convention).
    #[tokio::test]
    #[ignore]
    async fn test_insert_and_get_transaction_round_trips() {
        let Some(store) = setup_store().await else {
            return;
        };

        let tx = Transaction {
            id: format!("tx_test_{}", super::super::generate_uuid()),
            status: "pending_user_transfer_start".to_string(),
            asset_code: "USDC".to_string(),
            account: "GTESTACCOUNT".to_string(),
            amount_in: Some("100.0".to_string()),
            amount_out: None,
        };

        store.insert_transaction(tx.clone()).await.unwrap();

        let fetched = store
            .get_transaction(&tx.id)
            .await
            .unwrap()
            .expect("transaction should exist after insert");

        assert_eq!(fetched.id, tx.id);
        assert_eq!(fetched.status, tx.status);
        assert_eq!(fetched.asset_code, tx.asset_code);
        assert_eq!(fetched.account, tx.account);
        assert_eq!(fetched.amount_in, tx.amount_in);
        assert_eq!(fetched.amount_out, tx.amount_out);
    }

    #[tokio::test]
    #[ignore]
    async fn test_insert_transaction_upserts_on_conflict() {
        let Some(store) = setup_store().await else {
            return;
        };

        let id = format!("tx_test_{}", super::super::generate_uuid());
        let initial = Transaction {
            id: id.clone(),
            status: "pending_user_transfer_start".to_string(),
            asset_code: "USDC".to_string(),
            account: "GTESTACCOUNT".to_string(),
            amount_in: None,
            amount_out: None,
        };
        store.insert_transaction(initial).await.unwrap();

        let updated = Transaction {
            id: id.clone(),
            status: "completed".to_string(),
            asset_code: "USDC".to_string(),
            account: "GTESTACCOUNT".to_string(),
            amount_in: Some("100.0".to_string()),
            amount_out: Some("99.5".to_string()),
        };
        store.insert_transaction(updated).await.unwrap();

        let fetched = store.get_transaction(&id).await.unwrap().unwrap();
        assert_eq!(fetched.status, "completed");
        assert_eq!(fetched.amount_out, Some("99.5".to_string()));
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_transaction_returns_none_when_not_found() {
        let Some(store) = setup_store().await else {
            return;
        };

        let result = store.get_transaction("tx_does_not_exist").await.unwrap();
        assert!(result.is_none());
    }
}
