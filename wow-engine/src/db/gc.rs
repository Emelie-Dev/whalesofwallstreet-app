//! Background garbage collection for the `historical_routes` archive table.
//!
//! Every user quote/execution eventually gets archived into
//! `historical_routes` (see migration `002_add_historical_routes_and_bridge_quotas`),
//! which grows without bound unless something prunes it. This module runs a
//! periodic worker that deletes entries older than [`RETENTION_DAYS`], in
//! small batches so the delete never holds a long-lived lock against live
//! traffic on the table.

use crate::db::Database;
use chrono::{Duration as ChronoDuration, Utc};
use std::time::{Duration, Instant};

/// Historical route entries older than this are eligible for deletion.
pub const RETENTION_DAYS: i64 = 7;

/// How often a GC pass runs.
pub const GC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Maximum rows deleted per batch. Keeps each individual `DELETE` fast and
/// its lock window short, instead of one giant delete blocking reads/writes
/// on the table for the duration of the whole purge.
pub const GC_BATCH_SIZE: i64 = 5000;

/// Pause between batches so the purge doesn't monopolize the connection
/// pool back-to-back against live traffic.
const GC_BATCH_PAUSE: Duration = Duration::from_millis(100);

/// Runs the `historical_routes` GC forever, once every [`GC_INTERVAL`].
///
/// A failed pass (e.g. the database is temporarily unreachable) is logged
/// and the worker simply waits for the next interval to retry — it never
/// exits, so a transient outage doesn't permanently disable pruning.
pub async fn run_historical_routes_gc(db: Database) {
    loop {
        match purge_stale_historical_routes(&db, RETENTION_DAYS, GC_BATCH_SIZE).await {
            Ok((deleted_rows, elapsed)) => {
                tracing::info!(
                    deleted_rows,
                    duration_ms = elapsed.as_millis() as u64,
                    "historical_routes GC pass complete"
                );
            }
            Err(err) => {
                tracing::error!(
                    "historical_routes GC pass failed (will retry next interval): {err}"
                );
            }
        }
        tokio::time::sleep(GC_INTERVAL).await;
    }
}

/// Deletes `historical_routes` rows archived more than `retention_days` ago,
/// in batches of at most `batch_size` rows. Returns the total number of rows
/// deleted and how long the whole pass took.
///
/// Batching (rather than one `DELETE ... WHERE archived_at < cutoff`) keeps
/// each transaction's row lock short-lived: a single unbounded delete against
/// a multi-million-row table would hold locks long enough to stall
/// concurrent reads/writes for the duration of the whole purge.
async fn purge_stale_historical_routes(
    db: &Database,
    retention_days: i64,
    batch_size: i64,
) -> Result<(u64, Duration), sqlx::Error> {
    let cutoff = Utc::now() - ChronoDuration::days(retention_days);
    let start = Instant::now();
    let mut total_deleted: u64 = 0;

    loop {
        // Archived-at, not original-created-at: staleness is measured from
        // when the row entered this archive table, matching the indexed
        // `idx_historical_routes_archived_at` column.
        let result = sqlx::query(
            r#"
            DELETE FROM historical_routes
            WHERE id IN (
                SELECT id FROM historical_routes
                WHERE archived_at < $1
                LIMIT $2
            )
            "#,
        )
        .bind(cutoff)
        .bind(batch_size)
        .execute(db.pool())
        .await?;

        let deleted = result.rows_affected();
        total_deleted += deleted;

        // A short batch means we've caught up to the cutoff; a full batch
        // means there may be more stale rows waiting.
        if deleted < batch_size as u64 {
            break;
        }

        tokio::time::sleep(GC_BATCH_PAUSE).await;
    }

    Ok((total_deleted, start.elapsed()))
}
