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
    /// Returns the value registered with `App::provide::<T>()`.
    /// Fails with [`JobError::MissingState`] if `T` was not provided.
    pub fn state<T: Send + Sync + 'static>(&self) -> Result<&T, JobError> {
        self.state
            .get::<T>()
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
