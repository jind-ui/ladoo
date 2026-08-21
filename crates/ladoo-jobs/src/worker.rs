//! Worker loop — polls for jobs and dispatches them to registered handlers.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Semaphore};

use crate::error::JobStoreError;
use crate::registry::JobRegistry;
use crate::store::JobStore;

/// Polls the job store and executes claimed jobs.
///
/// A `Worker` repeatedly claims a batch of jobs from a [`JobStore`],
/// dispatches each one to the handler registered in a [`JobRegistry`],
/// and marks the job completed or failed based on the outcome. Up to
/// `concurrency` jobs run in parallel. If the store supports push
/// notifications (see [`JobStore::subscribe`]), the worker wakes
/// immediately on new jobs instead of waiting for the next poll.
pub struct Worker<S: JobStore> {
    store: Arc<S>,
    registry: Arc<JobRegistry>,
    poll_interval: Duration,
    batch_size: u32,
    concurrency: usize,
}

impl<S: JobStore> Worker<S> {
    /// Create a worker with default settings.
    ///
    /// Defaults: poll every 5s, batch size 10, concurrency 5.
    pub fn new(store: S, registry: JobRegistry) -> Self {
        Self {
            store: Arc::new(store),
            registry: Arc::new(registry),
            poll_interval: Duration::from_secs(5),
            batch_size: 10,
            concurrency: 5,
        }
    }

    /// Set the interval between polls.
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Set the maximum number of jobs to claim per poll.
    pub fn batch_size(mut self, n: u32) -> Self {
        self.batch_size = n;
        self
    }

    /// Set the maximum number of jobs executing in parallel.
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n;
        self
    }

    /// Run the worker loop until the shutdown signal fires.
    ///
    /// Each iteration claims up to `batch_size` jobs, spawns each onto
    /// its own task (bounded by a semaphore of size `concurrency`), then
    /// waits for whichever comes first: a store notification, the next
    /// poll tick, or a `true` on `shutdown`.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), JobStoreError> {
        let semaphore = Arc::new(Semaphore::new(self.concurrency));

        loop {
            let jobs = self.store.claim(self.batch_size).await?;

            for job in jobs {
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore is never closed");
                let store = self.store.clone();
                let registry = self.registry.clone();

                tokio::spawn(async move {
                    let result = registry.dispatch(&job.name, job.payload.clone()).await;
                    match result {
                        Ok(()) => {
                            let _ = store.complete(job.id).await;
                        }
                        Err(err) => {
                            let _ = store.fail(job.id, &err).await;
                        }
                    }
                    drop(permit);
                });
            }

            // Wait for next poll, notification, or shutdown.
            if let Some(mut notify_rx) = self.store.subscribe() {
                tokio::select! {
                    _ = notify_rx.changed() => { continue; }
                    _ = tokio::time::sleep(self.poll_interval) => {}
                    Ok(()) = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                }
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(self.poll_interval) => {}
                    Ok(()) = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{JobId, JobStatus, NewJob, QueuedJob};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex;

    struct MockStore {
        jobs: Mutex<Vec<QueuedJob>>,
        completed: Arc<Mutex<Vec<JobId>>>,
        failed: Arc<Mutex<Vec<(JobId, String)>>>,
        notify: Option<watch::Receiver<()>>,
    }

    impl MockStore {
        fn new(jobs: Vec<QueuedJob>) -> Self {
            Self {
                jobs: Mutex::new(jobs),
                completed: Arc::new(Mutex::new(vec![])),
                failed: Arc::new(Mutex::new(vec![])),
                notify: None,
            }
        }

        /// Shared handles into this store's outcome logs, for use in
        /// assertions after the store has been moved into a `Worker`.
        fn outcomes(&self) -> (Arc<Mutex<Vec<JobId>>>, Arc<Mutex<Vec<(JobId, String)>>>) {
            (self.completed.clone(), self.failed.clone())
        }
    }

    #[async_trait]
    impl JobStore for MockStore {
        async fn push(&self, _job: NewJob) -> Result<JobId, JobStoreError> {
            Ok(1)
        }
        async fn claim(&self, _limit: u32) -> Result<Vec<QueuedJob>, JobStoreError> {
            let mut jobs = self.jobs.lock().unwrap();
            let claimed = jobs.drain(..).collect();
            Ok(claimed)
        }
        async fn complete(&self, id: JobId) -> Result<(), JobStoreError> {
            self.completed.lock().unwrap().push(id);
            Ok(())
        }
        async fn fail(&self, id: JobId, error: &str) -> Result<(), JobStoreError> {
            self.failed.lock().unwrap().push((id, error.to_string()));
            Ok(())
        }
        fn subscribe(&self) -> Option<watch::Receiver<()>> {
            self.notify.clone()
        }
        async fn migrate(&self) -> Result<(), JobStoreError> {
            Ok(())
        }
    }

    fn make_queued_job(id: JobId, name: &str, payload: serde_json::Value) -> QueuedJob {
        QueuedJob {
            id,
            name: name.into(),
            payload,
            status: JobStatus::Running,
            attempts: 0,
            max_retries: 3,
            run_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_error: None,
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OkJob;
    impl crate::registry::PersistentJob for OkJob {
        fn name() -> &'static str {
            "ok_job"
        }
        fn handle(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct ErrJob;
    impl crate::registry::PersistentJob for ErrJob {
        fn name() -> &'static str {
            "err_job"
        }
        fn handle(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Err("boom".into()) })
        }
    }

    #[tokio::test]
    async fn worker_processes_and_completes_job() {
        let store = MockStore::new(vec![make_queued_job(1, "ok_job", serde_json::json!(null))]);
        let (completed, failed) = store.outcomes();

        let mut registry = JobRegistry::new();
        registry.register::<OkJob>();

        let worker = Worker::new(store, registry).poll_interval(Duration::from_millis(10));

        let (tx, rx) = watch::channel(false);

        let handle = tokio::spawn(async move { worker.run(rx).await });

        // Let the worker process the job.
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(true).unwrap();
        handle.await.unwrap().unwrap();

        assert_eq!(*completed.lock().unwrap(), vec![1]);
        assert!(failed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn worker_marks_failed_job_with_error_message() {
        let store = MockStore::new(vec![make_queued_job(2, "err_job", serde_json::json!(null))]);
        let (completed, failed) = store.outcomes();

        let mut registry = JobRegistry::new();
        registry.register::<ErrJob>();

        let worker = Worker::new(store, registry).poll_interval(Duration::from_millis(10));

        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move { worker.run(rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(true).unwrap();
        handle.await.unwrap().unwrap();

        assert!(completed.lock().unwrap().is_empty());
        let failed = failed.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, 2);
        assert!(failed[0].1.contains("boom"));
    }

    #[tokio::test]
    async fn worker_marks_unregistered_job_as_failed() {
        let store = MockStore::new(vec![make_queued_job(
            3,
            "no_such_job",
            serde_json::json!(null),
        )]);
        let (completed, failed) = store.outcomes();

        let worker =
            Worker::new(store, JobRegistry::new()).poll_interval(Duration::from_millis(10));

        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move { worker.run(rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(true).unwrap();
        handle.await.unwrap().unwrap();

        assert!(completed.lock().unwrap().is_empty());
        let failed = failed.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].1.contains("no handler registered"));
    }

    #[tokio::test]
    async fn worker_shutdown_on_signal() {
        let store = MockStore::new(vec![]);
        let registry = JobRegistry::new();

        let worker = Worker::new(store, registry).poll_interval(Duration::from_secs(60));

        let (tx, rx) = watch::channel(false);

        let handle = tokio::spawn(async move { worker.run(rx).await });

        // Send shutdown immediately; the worker should not wait out the
        // 60s poll interval.
        tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn worker_wakes_on_notify_instead_of_waiting_full_poll_interval() {
        let mut store = MockStore::new(vec![]);
        let (notify_tx, notify_rx) = watch::channel(());
        store.notify = Some(notify_rx);
        let (completed, _failed) = store.outcomes();

        let worker = Worker::new(store, JobRegistry::new()).poll_interval(Duration::from_secs(60));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(async move { worker.run(shutdown_rx).await });

        // Fire a couple of notifications quickly; if the worker is stuck
        // sleeping for 60s this test will time out.
        tokio::time::sleep(Duration::from_millis(20)).await;
        notify_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        shutdown_tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok());
        assert!(completed.lock().unwrap().is_empty());
    }
}
