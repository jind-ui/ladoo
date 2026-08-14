//! Job dispatcher and execution engine.

use std::sync::{Arc, OnceLock};

use crate::state::TypeMap;

use super::JobError;

/// Dispatches jobs for immediate in-process execution via `tokio::spawn`.
///
/// Provide it to the app with `App::provide(JobRunner::new())` and
/// extract it in handlers with `State<JobRunner>`.
#[derive(Clone)]
pub struct JobRunner {
    pub(crate) state: Arc<OnceLock<Arc<TypeMap>>>,
}

/// A handle to a spawned job. Await it to get the result.
pub struct JobHandle {
    pub(crate) inner: tokio::task::JoinHandle<Result<(), JobError>>,
}
