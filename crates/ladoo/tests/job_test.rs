#![cfg(all(feature = "macros", feature = "jobs"))]

//! Integration tests for the job queue subsystem.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ladoo::prelude::*;

struct IncrementJob {
    counter: Arc<AtomicU32>,
}

impl Job for IncrementJob {
    fn name(&self) -> &'static str {
        "increment"
    }

    fn config(&self) -> JobConfig {
        JobConfig::default()
    }

    async fn handle(&self, _ctx: &JobContext) -> std::result::Result<(), JobError> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn enqueue_job_from_handler() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let client = App::test()
        .provide(counter_clone)
        .provide(JobRunner::new())
        .get(
            "/work",
            |runner: State<JobRunner>, c: State<Arc<AtomicU32>>| async move {
                let job = IncrementJob {
                    counter: (*c).clone(),
                };
                runner.enqueue(job).wait().await.unwrap();
                "done"
            },
        )
        .into_client();

    let resp = client.get("/work").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "done");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

struct RetryJob {
    counter: Arc<AtomicU32>,
}

impl Job for RetryJob {
    fn name(&self) -> &'static str {
        "retry_job"
    }

    fn config(&self) -> JobConfig {
        JobConfig {
            max_retries: 2,
            timeout: Duration::from_secs(5),
            backoff: BackoffStrategy::Fixed(Duration::from_millis(1)),
        }
    }

    async fn handle(&self, _ctx: &JobContext) -> std::result::Result<(), JobError> {
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        if count < 2 {
            Err(JobError::failed(std::io::Error::other("not ready")))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn job_retries_then_succeeds() {
    let counter = Arc::new(AtomicU32::new(0));

    let client = App::test()
        .provide(counter.clone())
        .provide(JobRunner::new())
        .get(
            "/retry",
            |runner: State<JobRunner>, c: State<Arc<AtomicU32>>| async move {
                let job = RetryJob {
                    counter: (*c).clone(),
                };
                runner.enqueue(job).wait().await.unwrap();
                "ok"
            },
        )
        .into_client();

    let resp = client.get("/retry").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

struct StateAccessJob {
    result: Arc<AtomicU32>,
}

impl Job for StateAccessJob {
    fn name(&self) -> &'static str {
        "state_access"
    }

    async fn handle(&self, ctx: &JobContext) -> std::result::Result<(), JobError> {
        let val = ctx.state::<u32>()?;
        self.result.store(*val, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn job_accesses_provided_state() {
    let result = Arc::new(AtomicU32::new(0));

    let client = App::test()
        .provide(42_u32)
        .provide(result.clone())
        .provide(JobRunner::new())
        .get(
            "/state",
            |runner: State<JobRunner>, r: State<Arc<AtomicU32>>| async move {
                let job = StateAccessJob {
                    result: (*r).clone(),
                };
                runner.enqueue(job).wait().await.unwrap();
                "ok"
            },
        )
        .into_client();

    let resp = client.get("/state").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(result.load(Ordering::SeqCst), 42);
}

#[tokio::test]
async fn multiple_jobs_all_complete() {
    let counter = Arc::new(AtomicU32::new(0));

    let client = App::test()
        .provide(counter.clone())
        .provide(JobRunner::new())
        .get(
            "/multi",
            |runner: State<JobRunner>, c: State<Arc<AtomicU32>>| async move {
                let mut handles = Vec::new();
                for _ in 0..5 {
                    let job = IncrementJob {
                        counter: (*c).clone(),
                    };
                    handles.push(runner.enqueue(job));
                }
                for h in handles {
                    h.wait().await.unwrap();
                }
                "ok"
            },
        )
        .into_client();

    let resp = client.get("/multi").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn enqueue_forget_completes_without_error() {
    let counter = Arc::new(AtomicU32::new(0));

    let client = App::test()
        .provide(counter.clone())
        .provide(JobRunner::new())
        .get(
            "/forget",
            |runner: State<JobRunner>, c: State<Arc<AtomicU32>>| async move {
                let job = IncrementJob {
                    counter: (*c).clone(),
                };
                runner.enqueue_forget(job);
                "queued"
            },
        )
        .into_client();

    let resp = client.get("/forget").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "queued");

    // Give the background job time to complete
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn job_missing_state_returns_error() {
    struct NeedsDb;
    impl Job for NeedsDb {
        fn name(&self) -> &'static str {
            "needs_db"
        }
        async fn handle(&self, ctx: &JobContext) -> std::result::Result<(), JobError> {
            let _db = ctx.state::<String>()?;
            Ok(())
        }
    }

    let client = App::test()
        .provide(JobRunner::new())
        .get("/missing", |runner: State<JobRunner>| async move {
            let result = runner.enqueue(NeedsDb).wait().await;
            match result {
                Err(JobError::MissingState(_)) => "missing_state",
                _ => "unexpected",
            }
        })
        .into_client();

    let resp = client.get("/missing").send().await;
    assert_eq!(resp.text(), "missing_state");
}
