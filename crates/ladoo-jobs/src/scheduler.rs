//! Cron scheduler — inserts jobs on a schedule.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::watch;

use crate::registry::PersistentJob;
use crate::store::{JobStore, NewJob};

/// Error parsing a cron expression.
#[derive(Debug, thiserror::Error)]
#[error("invalid cron expression: {0}")]
pub struct CronError(pub String);

struct CronEntry {
    #[allow(dead_code)] // kept for diagnostics/dedup once the scheduler grows introspection
    name: String,
    schedule: Schedule,
    factory: Box<dyn Fn() -> NewJob + Send + Sync>,
}

/// Schedules recurring jobs by inserting them into the store
/// when their cron expression fires.
///
/// The scheduler checks every 60 seconds. It does not execute
/// jobs itself — it only inserts them. The [`Worker`](crate::Worker)
/// handles execution.
pub struct CronScheduler<S: JobStore> {
    store: Arc<S>,
    entries: Vec<CronEntry>,
}

impl<S: JobStore> CronScheduler<S> {
    /// Create a scheduler backed by the given store.
    pub fn new(store: S) -> Self {
        Self {
            store: Arc::new(store),
            entries: vec![],
        }
    }

    /// Register a recurring job with a cron expression.
    ///
    /// The job type must implement `Default` so the scheduler can
    /// create instances. The schedule uses standard 5-field cron
    /// syntax with an optional seconds field (6 fields).
    ///
    /// ```text
    /// "0 0 * * *"       — daily at midnight
    /// "*/5 * * * *"     — every 5 minutes
    /// "0 9 * * MON-FRI" — weekdays at 9am
    /// ```
    pub fn register<J: PersistentJob + Default>(
        &mut self,
        schedule: &str,
    ) -> Result<&mut Self, CronError> {
        let schedule = Schedule::from_str(schedule).map_err(|e| CronError(e.to_string()))?;

        let entry = CronEntry {
            name: J::name().to_string(),
            schedule,
            factory: Box::new(|| NewJob {
                name: J::name().to_string(),
                payload: serde_json::to_value(J::default()).unwrap_or_default(),
                max_retries: J::max_retries(),
                run_at: Utc::now(),
            }),
        };

        self.entries.push(entry);
        Ok(self)
    }

    /// Run the scheduler loop until the shutdown signal fires.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        loop {
            let now = Utc::now();

            for entry in &self.entries {
                if let Some(next) = entry.schedule.upcoming(Utc).next() {
                    // Fire if the next occurrence is within 60 seconds
                    let diff = next.signed_duration_since(now);
                    if diff.num_seconds() <= 60 {
                        let job = (entry.factory)();
                        let _ = self.store.push(job).await;
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                Ok(()) = shutdown.changed() => {
                    if *shutdown.borrow() { break; }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::JobStoreError;
    use crate::store::{JobId, QueuedJob};
    use async_trait::async_trait;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    struct RecordingStore {
        pushed: Mutex<Vec<NewJob>>,
    }

    impl RecordingStore {
        fn new() -> Self {
            Self {
                pushed: Mutex::new(vec![]),
            }
        }
        fn pushed_count(&self) -> usize {
            self.pushed.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl JobStore for RecordingStore {
        async fn push(&self, job: NewJob) -> Result<JobId, JobStoreError> {
            self.pushed.lock().unwrap().push(job);
            Ok(1)
        }
        async fn claim(&self, _: u32) -> Result<Vec<QueuedJob>, JobStoreError> {
            Ok(vec![])
        }
        async fn complete(&self, _: JobId) -> Result<(), JobStoreError> {
            Ok(())
        }
        async fn fail(&self, _: JobId, _: &str) -> Result<(), JobStoreError> {
            Ok(())
        }
        async fn migrate(&self) -> Result<(), JobStoreError> {
            Ok(())
        }
    }

    #[derive(Default, serde::Serialize, serde::Deserialize)]
    struct TickJob;
    impl PersistentJob for TickJob {
        fn name() -> &'static str {
            "tick"
        }
        fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn register_valid_cron() {
        let store = RecordingStore::new();
        let mut scheduler = CronScheduler::new(store);
        let result = scheduler.register::<TickJob>("0 * * * * *");
        assert!(result.is_ok());
    }

    #[test]
    fn register_invalid_cron() {
        let store = RecordingStore::new();
        let mut scheduler = CronScheduler::new(store);
        let result = scheduler.register::<TickJob>("not a cron");
        assert!(result.is_err());
    }

    #[test]
    fn register_returns_mut_ref_for_chaining() {
        let store = RecordingStore::new();
        let mut scheduler = CronScheduler::new(store);
        let result = scheduler
            .register::<TickJob>("0 * * * * *")
            .and_then(|s| s.register::<TickJob>("0 0 * * * *"));
        assert!(result.is_ok());
        assert_eq!(scheduler.entries.len(), 2);
    }

    #[tokio::test]
    async fn scheduler_shuts_down() {
        let store = RecordingStore::new();
        let scheduler = CronScheduler::new(store);

        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            scheduler.run(rx).await;
        });

        tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn scheduler_pushes_due_job_on_run() {
        let store = RecordingStore::new();
        let mut scheduler = CronScheduler::new(store);
        // Fires every second, so it is always within the 60s window.
        scheduler.register::<TickJob>("* * * * * *").unwrap();

        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            scheduler.run(rx).await;
            scheduler
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        let scheduler = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("scheduler should shut down promptly")
            .unwrap();

        assert!(scheduler.store.pushed_count() >= 1);
    }
}
