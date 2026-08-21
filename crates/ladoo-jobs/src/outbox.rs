//! Transactional outbox — enqueue a job within an existing DB transaction.
//!
//! These functions write directly to the `_ladoo_jobs` table using an
//! already-open `sqlx` transaction, bypassing [`crate::store::JobStore`].
//! The job row is only visible to workers once the caller commits the
//! transaction; if it rolls back, the job is never created. This lets a
//! job be enqueued atomically alongside other writes (e.g. "create the
//! order row and enqueue the confirmation email in the same transaction").

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::Utc;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use crate::error::JobStoreError;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use crate::registry::PersistentJob;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use crate::store::JobId;

/// Enqueue a job within a Postgres transaction.
///
/// The job row is only visible to workers after the transaction commits.
/// If the transaction rolls back, the job is never created.
#[cfg(feature = "postgres")]
pub async fn enqueue_outbox_pg<J: PersistentJob>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &J,
) -> Result<JobId, JobStoreError> {
    use sqlx::Row;

    let payload = serde_json::to_value(job).map_err(JobStoreError::Serialization)?;
    let now = Utc::now();

    let row = sqlx::query(
        "INSERT INTO _ladoo_jobs (name, payload, status, max_retries, run_at, created_at, updated_at) \
         VALUES ($1, $2, 'pending', $3, $4, $4, $4) RETURNING id",
    )
    .bind(J::name())
    .bind(&payload)
    .bind(J::max_retries() as i32)
    .bind(now)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|e| JobStoreError::Database(e.to_string()))?;

    Ok(row.get::<i64, _>("id"))
}

/// Enqueue a job within a SQLite transaction.
///
/// The job row is only visible to workers after the transaction commits.
/// If the transaction rolls back, the job is never created.
#[cfg(feature = "sqlite")]
pub async fn enqueue_outbox_sqlite<J: PersistentJob>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job: &J,
) -> Result<JobId, JobStoreError> {
    use sqlx::Row;

    let payload = serde_json::to_value(job).map_err(JobStoreError::Serialization)?;
    let now = Utc::now().to_rfc3339();

    let row = sqlx::query(
        "INSERT INTO _ladoo_jobs (name, payload, status, max_retries, run_at, created_at, updated_at) \
         VALUES (?, ?, 'pending', ?, ?, ?, ?) RETURNING id",
    )
    .bind(J::name())
    .bind(payload.to_string())
    .bind(J::max_retries() as i64)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|e| JobStoreError::Database(e.to_string()))?;

    Ok(row.get::<i64, _>("id"))
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStore;
    use crate::store::JobStore;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::future::Future;
    use std::pin::Pin;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OutboxJob {
        order_id: i64,
    }

    impl PersistentJob for OutboxJob {
        fn name() -> &'static str {
            "outbox_job"
        }
        fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    async fn test_pool() -> sqlx::SqlitePool {
        SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn outbox_commit_makes_job_visible() {
        let pool = test_pool().await;
        let store = SqliteStore::new(pool.clone()).unwrap();
        store.migrate().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let id = enqueue_outbox_sqlite(&mut tx, &OutboxJob { order_id: 42 })
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let claimed = store.claim(10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, id);
    }

    #[tokio::test]
    async fn outbox_rollback_discards_job() {
        let pool = test_pool().await;
        let store = SqliteStore::new(pool.clone()).unwrap();
        store.migrate().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        enqueue_outbox_sqlite(&mut tx, &OutboxJob { order_id: 99 })
            .await
            .unwrap();
        tx.rollback().await.unwrap();

        let claimed = store.claim(10).await.unwrap();
        assert!(claimed.is_empty());
    }
}
