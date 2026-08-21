//! Core types and the [`JobStore`] trait.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::JobStoreError;
use crate::registry::PersistentJob;

/// Unique identifier for a queued job.
pub type JobId = i64;

/// Current state of a queued job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    /// Ready to be claimed by a worker.
    Pending,
    /// Currently being executed.
    Running,
    /// Finished successfully.
    Completed,
    /// All retries exhausted.
    Failed,
    /// Waiting for its `run_at` time (delay / at).
    Scheduled,
}

impl JobStatus {
    /// Convert to the string stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Scheduled => "scheduled",
        }
    }

    /// Parse from the string stored in the database.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "scheduled" => Some(Self::Scheduled),
            _ => None,
        }
    }
}

/// A job in the queue with all metadata.
#[derive(Debug, Clone)]
pub struct QueuedJob {
    /// Unique identifier.
    pub id: JobId,
    /// Job type name (matches `Job::name()`).
    pub name: String,
    /// Serialized job data.
    pub payload: serde_json::Value,
    /// Current status.
    pub status: JobStatus,
    /// Number of execution attempts so far.
    pub attempts: u32,
    /// Maximum number of retries allowed.
    pub max_retries: u32,
    /// Earliest time the job should be executed.
    pub run_at: DateTime<Utc>,
    /// When the job was enqueued.
    pub created_at: DateTime<Utc>,
    /// Last status change.
    pub updated_at: DateTime<Utc>,
    /// Error message from the last failed attempt.
    pub last_error: Option<String>,
}

/// Data required to create a new job.
pub struct NewJob {
    /// Job type name.
    pub name: String,
    /// Serialized job data.
    pub payload: serde_json::Value,
    /// Maximum number of retries.
    pub max_retries: u32,
    /// Earliest time to execute.
    pub run_at: DateTime<Utc>,
}

/// Trait for database backends that store and manage jobs.
///
/// Implement this for custom backends (DynamoDB, CockroachDB, etc.).
/// Built-in implementations: `PostgresStore`, `SqliteStore`.
#[async_trait]
pub trait JobStore: Send + Sync + 'static {
    /// Insert a job into the queue.
    async fn push(&self, job: NewJob) -> Result<JobId, JobStoreError>;

    /// Claim up to `limit` pending jobs whose `run_at <= now`.
    ///
    /// Must be atomic — on Postgres use `FOR UPDATE SKIP LOCKED`,
    /// on SQLite use single-writer serialization.
    async fn claim(&self, limit: u32) -> Result<Vec<QueuedJob>, JobStoreError>;

    /// Mark a job as completed.
    async fn complete(&self, id: JobId) -> Result<(), JobStoreError>;

    /// Mark a job as failed, record the error, and schedule a retry
    /// if attempts remain.
    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobStoreError>;

    /// Optional notification channel for instant wake on new jobs.
    ///
    /// Returns `None` if the backend does not support push notifications.
    /// The worker falls back to polling when this returns `None`.
    fn subscribe(&self) -> Option<tokio::sync::watch::Receiver<()>> {
        None
    }

    /// Run schema migrations (create tables and indexes).
    async fn migrate(&self) -> Result<(), JobStoreError>;
}

/// Convenience enqueue methods for any [`JobStore`].
///
/// These build a [`NewJob`] from a [`PersistentJob`] and delegate to
/// [`JobStore::push`]. Implemented as a blanket extension trait so every
/// `JobStore` gets `enqueue`, `enqueue_delayed`, and `enqueue_at` for free.
#[async_trait]
pub trait JobStoreExt: JobStore {
    /// Enqueue a job for immediate execution.
    async fn enqueue<J: PersistentJob>(&self, job: &J) -> Result<JobId, JobStoreError> {
        let new_job = NewJob {
            name: J::name().to_string(),
            payload: serde_json::to_value(job).map_err(JobStoreError::Serialization)?,
            max_retries: J::max_retries(),
            run_at: Utc::now(),
        };
        self.push(new_job).await
    }

    /// Enqueue a job to run after a delay.
    async fn enqueue_delayed<J: PersistentJob>(
        &self,
        job: &J,
        delay: Duration,
    ) -> Result<JobId, JobStoreError> {
        let run_at = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
        let new_job = NewJob {
            name: J::name().to_string(),
            payload: serde_json::to_value(job).map_err(JobStoreError::Serialization)?,
            max_retries: J::max_retries(),
            run_at,
        };
        self.push(new_job).await
    }

    /// Enqueue a job to run at a specific time.
    async fn enqueue_at<J: PersistentJob>(
        &self,
        job: &J,
        at: DateTime<Utc>,
    ) -> Result<JobId, JobStoreError> {
        let new_job = NewJob {
            name: J::name().to_string(),
            payload: serde_json::to_value(job).map_err(JobStoreError::Serialization)?,
            max_retries: J::max_retries(),
            run_at: at,
        };
        self.push(new_job).await
    }
}

/// Blanket implementation — every [`JobStore`] automatically gets [`JobStoreExt`].
impl<S: JobStore> JobStoreExt for S {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn job_status_roundtrip() {
        let statuses = [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Scheduled,
        ];
        for status in &statuses {
            let s = status.as_str();
            let parsed = JobStatus::from_str(s).expect("should parse");
            assert_eq!(&parsed, status);
        }
    }

    #[test]
    fn job_status_from_str_unknown_returns_none() {
        assert!(JobStatus::from_str("unknown").is_none());
    }

    #[test]
    fn new_job_fields() {
        let job = NewJob {
            name: "test".into(),
            payload: serde_json::json!({"key": "value"}),
            max_retries: 3,
            run_at: Utc::now(),
        };
        assert_eq!(job.name, "test");
        assert_eq!(job.max_retries, 3);
    }
}
