//! Job execution context.

use std::sync::Arc;

use crate::state::TypeMap;

use super::JobError;

/// Execution context passed to [`Job::handle`](super::Job::handle).
///
/// Provides access to the application's DI container (the same values
/// registered with `App::provide()`) and metadata about the current
/// execution attempt.
pub struct JobContext {
    pub(crate) state: Arc<TypeMap>,
    pub(crate) attempt: u32,
    pub(crate) job_name: &'static str,
}

impl JobContext {
    /// Pull a dependency from the DI container.
    ///
    /// Returns the value registered with `App::provide::<T>()`, shared
    /// behind an `Arc`.
    /// Fails with [`JobError::MissingState`] if `T` was not provided.
    pub fn state<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, JobError> {
        self.state
            .get_shared::<T>()
            .ok_or_else(|| JobError::MissingState(std::any::type_name::<T>().to_string()))
    }

    /// Current attempt number (1 = first try, 2 = first retry, etc.)
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Name of the job being executed.
    pub fn job_name(&self) -> &'static str {
        self.job_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_returns_provided_type() {
        let mut map = TypeMap::new();
        map.insert_shared(42_u32);
        let ctx = JobContext {
            state: Arc::new(map),
            attempt: 1,
            job_name: "test_job",
        };
        assert_eq!(*ctx.state::<u32>().unwrap(), 42);
    }

    #[test]
    fn state_returns_error_for_missing_type() {
        let ctx = JobContext {
            state: Arc::new(TypeMap::new()),
            attempt: 1,
            job_name: "test_job",
        };
        let err = ctx.state::<u32>().unwrap_err();
        assert!(matches!(err, JobError::MissingState(_)));
        assert!(err.to_string().contains("u32"));
    }

    #[test]
    fn attempt_returns_current_attempt() {
        let ctx = JobContext {
            state: Arc::new(TypeMap::new()),
            attempt: 3,
            job_name: "test_job",
        };
        assert_eq!(ctx.attempt(), 3);
    }

    #[test]
    fn job_name_returns_name() {
        let ctx = JobContext {
            state: Arc::new(TypeMap::new()),
            attempt: 1,
            job_name: "send_email",
        };
        assert_eq!(ctx.job_name(), "send_email");
    }
}
