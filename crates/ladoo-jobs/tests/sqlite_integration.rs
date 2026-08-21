//! End-to-end test: enqueue -> worker picks up -> handler runs -> job marked complete.

#![cfg(feature = "sqlite")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use chrono::Utc;
use ladoo_jobs::{
    CronScheduler, JobRegistry, JobStore, JobStoreExt, NewJob, PersistentJob, SqliteStore, Worker,
};
use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::watch;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct CountingJob {
    increment: u32,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl PersistentJob for CountingJob {
    fn name() -> &'static str {
        "counting_job"
    }
    fn max_retries() -> u32 {
        0
    }
    fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            COUNTER.fetch_add(self.increment, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn full_pipeline_enqueue_process_complete() {
    COUNTER.store(0, Ordering::SeqCst);

    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let store = SqliteStore::new(pool).unwrap();
    store.migrate().await.unwrap();

    // Enqueue 3 jobs using convenience method
    store.enqueue(&CountingJob { increment: 10 }).await.unwrap();
    store.enqueue(&CountingJob { increment: 20 }).await.unwrap();
    store.enqueue(&CountingJob { increment: 5 }).await.unwrap();

    // Set up worker
    let mut registry = JobRegistry::new();
    registry.register::<CountingJob>();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let worker = Worker::new(store, registry)
        .poll_interval(Duration::from_millis(10))
        .batch_size(10)
        .concurrency(2);

    let handle = tokio::spawn(async move { worker.run(shutdown_rx).await });

    // Wait for jobs to be processed
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Shut down
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap().unwrap();

    // All 3 jobs should have been processed: 10 + 20 + 5 = 35
    assert_eq!(COUNTER.load(Ordering::SeqCst), 35);
}

#[tokio::test]
async fn delayed_job_not_processed_immediately() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let store = SqliteStore::new(pool).unwrap();
    store.migrate().await.unwrap();

    // Enqueue a job delayed by 1 hour
    store
        .enqueue_delayed(&CountingJob { increment: 100 }, Duration::from_secs(3600))
        .await
        .unwrap();

    // Worker should find nothing to claim
    let claimed = store.claim(10).await.unwrap();
    assert!(claimed.is_empty());
}

#[tokio::test]
async fn enqueue_at_specific_time() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let store = SqliteStore::new(pool).unwrap();
    store.migrate().await.unwrap();

    // Enqueue for right now
    let now = Utc::now();
    store
        .enqueue_at(&CountingJob { increment: 1 }, now)
        .await
        .unwrap();

    let claimed = store.claim(10).await.unwrap();
    assert_eq!(claimed.len(), 1);
}

/// Compile-time / smoke check that `CronScheduler` and `NewJob` are part of
/// the crate's public API surface, even though the pipeline tests above
/// don't otherwise exercise them.
#[tokio::test]
async fn cron_scheduler_and_new_job_are_part_of_public_api() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let store = SqliteStore::new(pool).unwrap();
    store.migrate().await.unwrap();

    let mut scheduler = CronScheduler::new(store);
    scheduler
        .register::<CountingJob>("0 0 * * * *")
        .expect("valid cron expression");

    let _new_job = NewJob {
        name: CountingJob::name().to_string(),
        payload: serde_json::json!({ "increment": 0 }),
        max_retries: CountingJob::max_retries(),
        run_at: Utc::now(),
    };
}
