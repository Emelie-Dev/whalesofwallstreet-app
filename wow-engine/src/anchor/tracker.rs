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
