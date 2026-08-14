#![cfg(all(feature = "macros", feature = "jobs"))]

use ladoo::job::{BackoffStrategy, Job, JobConfig, JobContext, JobError, JobRunner};
use ladoo::Job as DeriveJob;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(DeriveJob)]
struct DefaultsJob;

impl DefaultsJob {
    async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
        Ok(())
    }
}

#[derive(DeriveJob)]
#[job(retries = 3, timeout = "5m", backoff = "fixed")]
struct SendWelcomeEmail {
    user_id: i64,
}

impl SendWelcomeEmail {
    async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
        let _ = self.user_id;
        Ok(())
    }
}

#[test]
fn derive_defaults_snake_case_name() {
    let job = DefaultsJob;
    assert_eq!(job.name(), "defaults_job");
}

#[test]
fn derive_defaults_config() {
    let job = DefaultsJob;
    let config = job.config();
    assert_eq!(config.max_retries, 0);
    assert_eq!(config.timeout, Duration::from_secs(30));
    assert!(matches!(config.backoff, BackoffStrategy::Exponential { .. }));
}

#[test]
fn derive_name_is_snake_case_of_struct_name() {
    let job = SendWelcomeEmail { user_id: 1 };
    assert_eq!(job.name(), "send_welcome_email");
}

#[test]
fn derive_config_reads_job_attributes() {
    let job = SendWelcomeEmail { user_id: 1 };
    let config: JobConfig = job.config();
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.timeout, Duration::from_secs(300));
    assert!(matches!(config.backoff, BackoffStrategy::Fixed(d) if d == Duration::from_secs(1)));
}

#[tokio::test]
async fn derived_job_runs_via_job_runner() {
    let runner = JobRunner::new();
    let handle = runner.enqueue(SendWelcomeEmail { user_id: 42 });
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn derived_job_handle_delegates_to_inherent_method() {
    struct CountingState {
        calls: AtomicU32,
    }

    #[derive(DeriveJob)]
    struct CountJob {
        state: Arc<CountingState>,
    }

    impl CountJob {
        async fn handle(&self, _ctx: &JobContext) -> Result<(), JobError> {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let state = Arc::new(CountingState {
        calls: AtomicU32::new(0),
    });
    let runner = JobRunner::new();
    let handle = runner.enqueue(CountJob {
        state: state.clone(),
    });
    handle.wait().await.unwrap();
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

/// Regression test: `ladoo::prelude` re-exports a single-type-param `Result<T>`
/// alias that shadows `std::result::Result<T, E>`. The `#[derive(Job)]` macro
/// must fully qualify `::core::result::Result` (and `::ladoo::` paths) in its
/// generated code so it still compiles when the caller has `use
/// ladoo::prelude::*` in scope.
mod prelude_compat {
    use ladoo::prelude::*;

    #[derive(Job)]
    #[job(retries = 2)]
    struct PreludeJob;

    impl PreludeJob {
        async fn handle(&self, _ctx: &JobContext) -> std::result::Result<(), JobError> {
            Ok(())
        }
    }

    #[test]
    fn derive_compiles_with_prelude_glob_import() {
        let job = PreludeJob;
        assert_eq!(job.name(), "prelude_job");
        assert_eq!(job.config().max_retries, 2);
    }
}
