//! Postgres backend for the job queue.

pub(crate) mod migrations;
pub(crate) mod notify;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::postgres::PgPool;
use sqlx::{Executor, Row};
use tokio::sync::watch;

use crate::error::JobStoreError;
use crate::store::{JobId, JobStatus, JobStore, NewJob, QueuedJob};

/// Postgres-backed [`JobStore`] implementation.
///
/// Uses `FOR UPDATE SKIP LOCKED` for atomic claim operations, so multiple
/// worker processes can poll the same table concurrently without
/// double-claiming a job. Spawns a background `LISTEN/NOTIFY` listener
/// so the worker wakes immediately when new jobs are inserted instead
/// of waiting for the next poll interval.
pub struct PostgresStore {
    pool: PgPool,
    notify_rx: watch::Receiver<()>,
}

impl PostgresStore {
    /// Connect to Postgres and start the `LISTEN/NOTIFY` listener.
    ///
    /// Call [`JobStore::migrate`] afterwards to create the schema.
    pub async fn new(pool: PgPool) -> Result<Self, JobStoreError> {
        let notify_rx = notify::spawn_listener(pool.clone()).await?;
        Ok(Self { pool, notify_rx })
    }
}

#[async_trait]
impl JobStore for PostgresStore {
    async fn push(&self, job: NewJob) -> Result<JobId, JobStoreError> {
        let status = if job.run_at > Utc::now() {
            JobStatus::Scheduled.as_str()
        } else {
            JobStatus::Pending.as_str()
        };

        let row = sqlx::query(
            "INSERT INTO _ladoo_jobs (name, payload, status, max_retries, run_at) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(&job.name)
        .bind(&job.payload)
        .bind(status)
        .bind(job.max_retries as i32)
        .bind(job.run_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        Ok(row.get::<i64, _>("id"))
    }

    async fn claim(&self, limit: u32) -> Result<Vec<QueuedJob>, JobStoreError> {
        let now = Utc::now();
        let worker_id = format!("worker-{}", std::process::id());

        let rows = sqlx::query(
            "UPDATE _ladoo_jobs SET status = 'running', locked_by = $1, locked_at = $2, updated_at = $2 \
             WHERE id IN ( \
                SELECT id FROM _ladoo_jobs \
                WHERE status IN ('pending', 'scheduled') AND run_at <= $3 \
                ORDER BY run_at ASC \
                FOR UPDATE SKIP LOCKED \
                LIMIT $4 \
             ) \
             RETURNING id, name, payload, status, attempts, max_retries, \
                       run_at, created_at, updated_at, last_error",
        )
        .bind(&worker_id)
        .bind(now)
        .bind(now)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobStoreError::Database(e.to_string()))?;

        let jobs = rows
            .iter()
            .map(|row| QueuedJob {
                id: row.get::<i64, _>("id"),
                name: row.get("name"),
                payload: row.get("payload"),
                status: JobStatus::Running,
                attempts: row.get::<i32, _>("attempts") as u32,
                max_retries: row.get::<i32, _>("max_retries") as u32,
                run_at: row.get("run_at"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                last_error: row.get("last_error"),
            })
            .collect();

        Ok(jobs)
    }

    async fn complete(&self, id: JobId) -> Result<(), JobStoreError> {
        let result = sqlx::query(
            "UPDATE _ladoo_jobs SET status = 'completed', locked_by = NULL, locked_at = NULL, \
             updated_at = NOW() WHERE id = $1",
        )
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
        let row = sqlx::query("SELECT attempts, max_retries FROM _ladoo_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?
            .ok_or(JobStoreError::NotFound(id))?;

        let attempts = row.get::<i32, _>("attempts") as u32 + 1;
        let max_retries = row.get::<i32, _>("max_retries") as u32;

        if attempts <= max_retries {
            // Schedule retry with exponential backoff: base 1s * 2^attempt, max 60s
            let delay_secs = std::cmp::min(2i64.saturating_pow(attempts - 1), 60);
            let next_run = Utc::now() + chrono::Duration::seconds(delay_secs);
            sqlx::query(
                "UPDATE _ladoo_jobs SET status = 'pending', attempts = $1, last_error = $2, \
                 run_at = $3, locked_by = NULL, locked_at = NULL, updated_at = NOW() WHERE id = $4",
            )
            .bind(attempts as i32)
            .bind(error)
            .bind(next_run)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        } else {
            sqlx::query(
                "UPDATE _ladoo_jobs SET status = 'failed', attempts = $1, last_error = $2, \
                 locked_by = NULL, locked_at = NULL, updated_at = NOW() WHERE id = $3",
            )
            .bind(attempts as i32)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        }

        Ok(())
    }

    fn subscribe(&self) -> Option<watch::Receiver<()>> {
        Some(self.notify_rx.clone())
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
        self.pool
            .execute(migrations::NOTIFY_FUNCTION)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        self.pool
            .execute(migrations::NOTIFY_TRIGGER)
            .await
            .map_err(|e| JobStoreError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Postgres tests require a running Postgres instance.
    // Set DATABASE_URL=postgres://user:pass@localhost/test to run them.
    // They are skipped unless the database is available.

    async fn maybe_store() -> Option<PostgresStore> {
        let url = std::env::var("DATABASE_URL").ok()?;
        if !url.starts_with("postgres") {
            return None;
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .ok()?;
        // Clean up from prior runs
        let _ = sqlx::query("DROP TABLE IF EXISTS _ladoo_jobs CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS _ladoo_cron_schedules CASCADE")
            .execute(&pool)
            .await;
        let store = PostgresStore::new(pool).await.ok()?;
        store.migrate().await.ok()?;
        Some(store)
    }

    #[tokio::test]
    async fn postgres_push_and_claim() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let id = store
            .push(NewJob {
                name: "pg_test".into(),
                payload: serde_json::json!({"x": 1}),
                max_retries: 2,
                run_at: Utc::now(),
            })
            .await
            .unwrap();

        let claimed = store.claim(10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, id);
        assert_eq!(claimed[0].name, "pg_test");
    }

    #[tokio::test]
    async fn postgres_claim_respects_limit() {
        let Some(store) = maybe_store().await else {
            return;
        };

        for i in 0..5 {
            store
                .push(NewJob {
                    name: format!("pg_job_{i}"),
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
    async fn postgres_claim_skips_future_jobs() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let future = Utc::now() + chrono::Duration::hours(1);
        store
            .push(NewJob {
                name: "pg_future_job".into(),
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
    async fn postgres_complete_and_fail() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let id1 = store
            .push(NewJob {
                name: "complete_test".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: Utc::now(),
            })
            .await
            .unwrap();

        let id2 = store
            .push(NewJob {
                name: "fail_test".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: Utc::now(),
            })
            .await
            .unwrap();

        store.claim(10).await.unwrap();
        store.complete(id1).await.unwrap();
        store.fail(id2, "boom").await.unwrap();

        // id1 should be completed, id2 should be failed (max_retries=0)
        let row1: (String,) = sqlx::query_as("SELECT status FROM _ladoo_jobs WHERE id = $1")
            .bind(id1)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(row1.0, "completed");

        let row2: (String,) = sqlx::query_as("SELECT status FROM _ladoo_jobs WHERE id = $1")
            .bind(id2)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(row2.0, "failed");
    }

    #[tokio::test]
    async fn postgres_complete_not_found() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let result = store.complete(999_999_999).await;
        assert!(matches!(result, Err(JobStoreError::NotFound(999_999_999))));
    }

    #[tokio::test]
    async fn postgres_fail_schedules_retry() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let id = store
            .push(NewJob {
                name: "pg_retry_me".into(),
                payload: serde_json::json!({}),
                max_retries: 3,
                run_at: Utc::now(),
            })
            .await
            .unwrap();
        store.claim(1).await.unwrap();

        store.fail(id, "first error").await.unwrap();

        let row: (String, i32, Option<String>) = sqlx::query_as(
            "SELECT status, attempts, last_error FROM _ladoo_jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "pending");
        assert_eq!(row.1, 1);
        assert_eq!(row.2.as_deref(), Some("first error"));
    }

    #[tokio::test]
    async fn postgres_subscribe_returns_some() {
        let Some(store) = maybe_store().await else {
            return;
        };
        assert!(store.subscribe().is_some());
    }

    #[tokio::test]
    async fn postgres_notify_fires_on_push() {
        let Some(store) = maybe_store().await else {
            return;
        };

        let mut rx = store.subscribe().expect("postgres always subscribes");

        // Drain any signal already queued before we start watching.
        rx.mark_unchanged();

        store
            .push(NewJob {
                name: "notify_test".into(),
                payload: serde_json::json!({}),
                max_retries: 0,
                run_at: Utc::now(),
            })
            .await
            .unwrap();

        // The INSERT trigger should fire pg_notify, which the background
        // listener forwards onto the watch channel within a few seconds.
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.changed())
            .await
            .expect("notification should arrive after push")
            .expect("watch sender should not be dropped");
    }
}
