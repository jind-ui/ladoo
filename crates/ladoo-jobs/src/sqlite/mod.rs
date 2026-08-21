//! SQLite backend for the job queue.

pub(crate) mod migrations;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;
use sqlx::{Executor, Row};

use crate::error::JobStoreError;
use crate::store::{JobId, JobStatus, JobStore, NewJob, QueuedJob};

/// SQLite-backed [`JobStore`] implementation.
///
/// Uses a single-writer model — `max_connections(1)` is recommended
/// for the pool to avoid `SQLITE_BUSY` under concurrent claims.
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Wrap an existing connection pool.
    pub fn new(pool: SqlitePool) -> Result<Self, JobStoreError> {
        Ok(Self { pool })
    }
}

#[async_trait]
impl JobStore for SqliteStore {
    async fn push(&self, job: NewJob) -> Result<JobId, JobStoreError> {
        let now = Utc::now().to_rfc3339();
        let run_at = job.run_at.to_rfc3339();
        let payload = job.payload.to_string();
        let status = if job.run_at > Utc::now() {
            JobStatus::Scheduled.as_str()
        } else {
            JobStatus::Pending.as_str()
        };

        let row = sqlx::query(
            "INSERT INTO _ladoo_jobs (name, payload, status, max_retries, run_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&job.name)
        .bind(&payload)
        .bind(status)
        .bind(job.max_retries as i64)
        .bind(&run_at)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        Ok(row.get::<i64, _>("id"))
    }

    async fn claim(&self, limit: u32) -> Result<Vec<QueuedJob>, JobStoreError> {
        let now = Utc::now().to_rfc3339();
        let worker_id = format!("worker-{}", std::process::id());

        // SQLite single-writer: UPDATE + SELECT in one step
        sqlx::query(
            "UPDATE _ladoo_jobs SET status = 'running', locked_by = ?, locked_at = ?, updated_at = ? \
             WHERE id IN ( \
                SELECT id FROM _ladoo_jobs \
                WHERE status IN ('pending', 'scheduled') AND run_at <= ? \
                ORDER BY run_at ASC LIMIT ? \
             )",
        )
        .bind(&worker_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        let rows = sqlx::query(
            "SELECT id, name, payload, status, attempts, max_retries, \
                    run_at, created_at, updated_at, last_error \
             FROM _ladoo_jobs WHERE locked_by = ? AND status = 'running' \
             ORDER BY run_at ASC",
        )
        .bind(&worker_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        let jobs = rows
            .iter()
            .map(|row| {
                let payload_str: String = row.get("payload");
                QueuedJob {
                    id: row.get::<i64, _>("id"),
                    name: row.get("name"),
                    payload: serde_json::from_str(&payload_str).unwrap_or_default(),
                    status: JobStatus::Running,
                    attempts: row.get::<i64, _>("attempts") as u32,
                    max_retries: row.get::<i64, _>("max_retries") as u32,
                    run_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("run_at"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    last_error: row.get("last_error"),
                }
            })
            .collect();

        Ok(jobs)
    }

    async fn complete(&self, id: JobId) -> Result<(), JobStoreError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE _ladoo_jobs SET status = 'completed', locked_by = NULL, locked_at = NULL, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(JobStoreError::NotFound(id));
        }
        Ok(())
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobStoreError> {
        let now = Utc::now().to_rfc3339();

        // Fetch current state
        let row = sqlx::query("SELECT attempts, max_retries, run_at FROM _ladoo_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?
            .ok_or(JobStoreError::NotFound(id))?;

        let attempts = row.get::<i64, _>("attempts") as u32 + 1;
        let max_retries = row.get::<i64, _>("max_retries") as u32;

        if attempts <= max_retries {
            // Schedule retry with exponential backoff: base 1s * 2^attempt, max 60s
            let delay_secs = std::cmp::min(2u64.saturating_pow(attempts - 1), 60);
            let next_run = (Utc::now() + chrono::Duration::seconds(delay_secs as i64)).to_rfc3339();
            sqlx::query(
                "UPDATE _ladoo_jobs SET status = 'pending', attempts = ?, last_error = ?, \
                 run_at = ?, locked_by = NULL, locked_at = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(attempts as i64)
            .bind(error)
            .bind(&next_run)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        } else {
            sqlx::query(
                "UPDATE _ladoo_jobs SET status = 'failed', attempts = ?, last_error = ?, \
                 locked_by = NULL, locked_at = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(attempts as i64)
            .bind(error)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        }

        Ok(())
    }

    async fn migrate(&self) -> Result<(), JobStoreError> {
        self.pool
            .execute(migrations::CREATE_JOBS_TABLE)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        self.pool
            .execute(migrations::CREATE_JOBS_INDEX)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        self.pool
            .execute(migrations::CREATE_CRON_TABLE)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_store() -> SqliteStore {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let store = SqliteStore::new(pool).unwrap();
        store.migrate().await.unwrap();
        store
    }

    #[tokio::test]
    async fn migrate_creates_tables() {
        let store = test_store().await;
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _ladoo_jobs")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(row.0, 0);
    }

    #[tokio::test]
    async fn push_and_claim_roundtrip() {
        let store = test_store().await;
        let job = NewJob {
            name: "test_job".into(),
            payload: serde_json::json!({"key": "val"}),
            max_retries: 3,
            run_at: Utc::now(),
        };
        let id = store.push(job).await.unwrap();
        assert!(id > 0);

        let claimed = store.claim(10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, id);
        assert_eq!(claimed[0].name, "test_job");
        assert_eq!(claimed[0].status, JobStatus::Running);
    }

    #[tokio::test]
    async fn claim_respects_limit() {
        let store = test_store().await;
        for i in 0..5 {
            store
                .push(NewJob {
                    name: format!("job_{i}"),
                    payload: serde_json::json!({}),
                    max_retries: 0,
                    run_at: Utc::now(),
                })
                .await
                .unwrap();
        }
        let claimed = store.claim(2).await.unwrap();
        assert_eq!(claimed.len(), 2);
    }

    #[tokio::test]
    async fn claim_skips_future_jobs() {
        let store = test_store().await;
        let future = Utc::now() + chrono::Duration::hours(1);
        store
            .push(NewJob {
                name: "future_job".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: future,
            })
            .await
            .unwrap();
        let claimed = store.claim(10).await.unwrap();
        assert!(claimed.is_empty());
    }

    #[tokio::test]
    async fn complete_marks_done() {
        let store = test_store().await;
        let id = store
            .push(NewJob {
                name: "complete_me".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: Utc::now(),
            })
            .await
            .unwrap();
        let claimed = store.claim(1).await.unwrap();
        assert_eq!(claimed.len(), 1);

        store.complete(id).await.unwrap();

        // Should not be claimable again
        let claimed = store.claim(10).await.unwrap();
        assert!(claimed.is_empty());
    }

    #[tokio::test]
    async fn complete_not_found() {
        let store = test_store().await;
        let result = store.complete(999).await;
        assert!(matches!(result, Err(JobStoreError::NotFound(999))));
    }

    #[tokio::test]
    async fn fail_schedules_retry() {
        let store = test_store().await;
        let id = store
            .push(NewJob {
                name: "retry_me".into(),
                payload: serde_json::json!({}),
                max_retries: 3,
                run_at: Utc::now(),
            })
            .await
            .unwrap();
        store.claim(1).await.unwrap();

        store.fail(id, "first error").await.unwrap();

        // Should be pending again (for retry)
        let row: (String, i64, String) =
            sqlx::query_as("SELECT status, attempts, last_error FROM _ladoo_jobs WHERE id = ?")
                .bind(id)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(row.0, "pending");
        assert_eq!(row.1, 1);
        assert_eq!(row.2, "first error");
    }

    #[tokio::test]
    async fn fail_marks_failed_after_max_retries() {
        let store = test_store().await;
        let id = store
            .push(NewJob {
                name: "exhaust_me".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: Utc::now(),
            })
            .await
            .unwrap();
        store.claim(1).await.unwrap();

        store.fail(id, "final error").await.unwrap();

        let row: (String,) = sqlx::query_as("SELECT status FROM _ladoo_jobs WHERE id = ?")
            .bind(id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(row.0, "failed");
    }

    #[tokio::test]
    async fn subscribe_returns_none() {
        let store = test_store().await;
        assert!(store.subscribe().is_none());
    }
}
