//! Job dispatcher and execution engine.

use std::sync::{Arc, OnceLock};

use crate::state::TypeMap;

use super::{Job, JobConfig, JobContext, JobError};

/// Dispatches jobs for immediate in-process execution via `tokio::spawn`.
///
/// Provide it to the app with `App::provide(JobRunner::new())` and
/// extract it in handlers with `State<JobRunner>`.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// App::new()
///     .provide(JobRunner::new())
///     .get("/work", |runner: State<JobRunner>| {
///         runner.enqueue(MyJob { id: 1 });
///         "queued"
///     });
/// ```
#[derive(Clone)]
pub struct JobRunner {
    state: Arc<OnceLock<Arc<TypeMap>>>,
}

impl JobRunner {
    /// Create a new runner. The app's state map is injected later
    /// during startup via [`App::run()`](crate::app::App::run) or
    /// [`App::into_client()`](crate::app::App::into_client).
    pub fn new() -> Self {
        Self {
            state: Arc::new(OnceLock::new()),
        }
    }

    /// Enqueue a job for immediate in-process execution.
    ///
    /// Returns a [`JobHandle`] that can be awaited to get the result.
    /// The job runs in a separate tokio task — this method returns
    /// immediately.
    pub fn enqueue<J: Job>(&self, job: J) -> JobHandle {
        let state = self.get_state();
        let inner = tokio::spawn(async move { run_job(&job, state).await });
        JobHandle { inner }
    }

    /// Fire-and-forget — spawns the job and discards the result.
    ///
    /// Errors are not propagated. Use [`enqueue`](Self::enqueue) if you
    /// need to track job completion or handle failures.
    pub fn enqueue_forget<J: Job>(&self, job: J) {
        let state = self.get_state();
        tokio::spawn(async move {
            let _ = run_job(&job, state).await;
        });
    }

    /// Inject the finalized state map. Called by the app during startup.
    pub(crate) fn initialize(&self, state: Arc<TypeMap>) {
        let _ = self.state.set(state);
    }

    /// Read the injected state, falling back to an empty map if the
    /// runner was never wired into an `App` (e.g. constructed and used
    /// directly in a unit test).
    fn get_state(&self) -> Arc<TypeMap> {
        self.state
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(TypeMap::new()))
    }
}

impl Default for JobRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a job to completion, retrying on retryable failures according to
/// its [`JobConfig`].
///
/// Attempt numbers are 1-indexed (1 = first try). The loop runs at most
/// `config.max_retries + 1` attempts. A [`JobError`] where
/// [`is_permanent`](JobError::is_permanent) returns `true` stops retries
/// immediately. A timed-out attempt is treated as a retryable failure
/// unless it is the final attempt, in which case it surfaces as
/// [`JobError::Timeout`].
async fn run_job<J: Job>(job: &J, state: Arc<TypeMap>) -> Result<(), JobError> {
    let config = job.config();
    let job_name = job.name();
    let total_attempts = config.max_retries + 1;

    for attempt in 1..=total_attempts {
        let ctx = JobContext {
            state: state.clone(),
            attempt,
            job_name,
        };

        let result = tokio::time::timeout(config.timeout, job.handle(&ctx)).await;
        let is_last_attempt = attempt == total_attempts;

        match result {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(err)) if err.is_permanent() => return Err(err),
            Ok(Err(err)) => {
                if is_last_attempt {
                    return Err(err);
                }
                sleep_before_retry(&config, attempt).await;
            }
            Err(_elapsed) => {
                if is_last_attempt {
                    return Err(JobError::Timeout);
                }
                sleep_before_retry(&config, attempt).await;
            }
        }
    }

    unreachable!("loop always returns before exhausting total_attempts")
}

/// Sleep for the backoff delay before the next retry.
///
/// `attempt` is the 1-indexed attempt that just failed; the backoff
/// strategy is 0-indexed by retry number, so it is passed `attempt - 1`.
async fn sleep_before_retry(config: &JobConfig, attempt: u32) {
    let delay = config.backoff.delay(attempt - 1);
    tokio::time::sleep(delay).await;
}

/// A handle to a spawned job.
///
/// Call [`wait()`](JobHandle::wait) to block until the job completes
/// and get its result. Dropping the handle does NOT cancel the job —
/// it continues running in the background.
pub struct JobHandle {
    inner: tokio::task::JoinHandle<Result<(), JobError>>,
}

impl JobHandle {
    /// Wait for the job to complete and return its result.
    ///
    /// Returns [`JobError::Failed`] wrapping the panic if the job task
    /// panicked instead of returning normally.
    pub async fn wait(self) -> Result<(), JobError> {
        match self.inner.await {
            Ok(result) => result,
            Err(join_err) => Err(JobError::Failed(Box::new(join_err))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    struct SuccessJob;
    impl Job for SuccessJob {
        fn name(&self) -> &'static str {
            "success"
        }
        async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
            Ok(())
        }
    }

    struct FailJob;
    impl Job for FailJob {
        fn name(&self) -> &'static str {
            "fail"
        }
        async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
            Err(JobError::failed(std::io::Error::other("boom")))
        }
    }

    struct PermanentRetryJob {
        counter: Arc<AtomicU32>,
    }
    impl Job for PermanentRetryJob {
        fn name(&self) -> &'static str {
            "perm_retry"
        }
        fn config(&self) -> JobConfig {
            JobConfig {
                max_retries: 5,
                timeout: Duration::from_secs(5),
                backoff: super::super::BackoffStrategy::Fixed(Duration::from_millis(1)),
            }
        }
        fn handle(
            &self,
            _ctx: &JobContext,
        ) -> impl std::future::Future<Output = Result<(), JobError>> + Send {
            let counter = self.counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(JobError::permanent(std::io::Error::other("stop")))
            }
        }
    }

    struct RetryCountingJob {
        counter: Arc<AtomicU32>,
    }
    impl Job for RetryCountingJob {
        fn name(&self) -> &'static str {
            "retry_counter"
        }
        fn config(&self) -> JobConfig {
            JobConfig {
                max_retries: 2,
                timeout: Duration::from_secs(5),
                backoff: super::super::BackoffStrategy::Fixed(Duration::from_millis(1)),
            }
        }
        fn handle(
            &self,
            _ctx: &JobContext,
        ) -> impl std::future::Future<Output = Result<(), JobError>> + Send {
            let counter = self.counter.clone();
            async move {
                let count = counter.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err(JobError::failed(std::io::Error::other("not yet")))
                } else {
                    Ok(())
                }
            }
        }
    }

    struct AlwaysFailsJob {
        counter: Arc<AtomicU32>,
    }
    impl Job for AlwaysFailsJob {
        fn name(&self) -> &'static str {
            "always_fails"
        }
        fn config(&self) -> JobConfig {
            JobConfig {
                max_retries: 2,
                timeout: Duration::from_secs(5),
                backoff: super::super::BackoffStrategy::Fixed(Duration::from_millis(1)),
            }
        }
        async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Err(JobError::failed(std::io::Error::other("still failing")))
        }
    }

    struct SlowJob;
    impl Job for SlowJob {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn config(&self) -> JobConfig {
            JobConfig {
                max_retries: 0,
                timeout: Duration::from_millis(10),
                backoff: super::super::BackoffStrategy::exponential_default(),
            }
        }
        async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    }

    fn initialized_runner() -> JobRunner {
        let runner = JobRunner::new();
        runner.initialize(Arc::new(TypeMap::new()));
        runner
    }

    #[tokio::test]
    async fn enqueue_success_returns_ok() {
        let runner = initialized_runner();
        let result = runner.enqueue(SuccessJob).wait().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn enqueue_fail_returns_err() {
        let runner = initialized_runner();
        let result = runner.enqueue(FailJob).wait().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn retries_on_failed_error() {
        let counter = Arc::new(AtomicU32::new(0));
        let runner = initialized_runner();
        let result = runner
            .enqueue(RetryCountingJob {
                counter: counter.clone(),
            })
            .wait()
            .await;
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retries_exhausted_returns_final_err() {
        let counter = Arc::new(AtomicU32::new(0));
        let runner = initialized_runner();
        let result = runner
            .enqueue(AlwaysFailsJob {
                counter: counter.clone(),
            })
            .wait()
            .await;
        assert!(result.is_err());
        // max_retries: 2 → 3 total attempts (1 initial + 2 retries), all consumed
        // before the loop gives up and returns the last attempt's error.
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_on_permanent() {
        let counter = Arc::new(AtomicU32::new(0));
        let runner = initialized_runner();
        let result = runner
            .enqueue(PermanentRetryJob {
                counter: counter.clone(),
            })
            .wait()
            .await;
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        let runner = initialized_runner();
        let result = runner.enqueue(SlowJob).wait().await;
        let err = result.unwrap_err();
        assert!(matches!(err, JobError::Timeout));
    }

    #[tokio::test]
    async fn enqueue_forget_does_not_panic() {
        let runner = initialized_runner();
        runner.enqueue_forget(FailJob);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn uninitialized_runner_does_not_panic() {
        let runner = JobRunner::new();
        let result = runner.enqueue(SuccessJob).wait().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn job_accesses_state_via_context() {
        let mut map = TypeMap::new();
        map.insert_shared(42_u32);
        let runner = JobRunner::new();
        runner.initialize(Arc::new(map));

        struct StateJob;
        impl Job for StateJob {
            fn name(&self) -> &'static str {
                "state_job"
            }
            async fn handle(&self, ctx: &JobContext) -> Result<(), JobError> {
                let val = ctx.state::<u32>()?;
                assert_eq!(*val, 42);
                Ok(())
            }
        }

        let result = runner.enqueue(StateJob).wait().await;
        assert!(result.is_ok());
    }
}
