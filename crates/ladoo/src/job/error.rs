//! Job execution errors.

use std::fmt;

/// Errors that can occur during job execution.
///
/// The variant determines retry behavior:
/// - `Failed` — retryable if attempts remain
/// - `Permanent` — never retried
/// - `MissingState` — never retried (configuration error)
/// - `Timeout` — retryable if attempts remain
#[derive(Debug)]
pub enum JobError {
    /// Job logic failed. The runner will retry if attempts remain.
    Failed(Box<dyn std::error::Error + Send + Sync>),
    /// Permanent failure. The runner will NOT retry.
    Permanent(Box<dyn std::error::Error + Send + Sync>),
    /// A required `State<T>` was not provided via `App::provide()`.
    MissingState(String),
    /// Job exceeded its configured timeout.
    Timeout,
}

impl JobError {
    /// Wrap any error as a retryable failure.
    pub fn failed(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Failed(Box::new(err))
    }

    /// Wrap any error as a permanent (non-retryable) failure.
    pub fn permanent(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Permanent(Box::new(err))
    }

    /// Whether this error should prevent further retries.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_) | Self::MissingState(_))
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(e) => write!(f, "job failed: {e}"),
            Self::Permanent(e) => write!(f, "job permanently failed: {e}"),
            Self::MissingState(name) => write!(f, "missing state: {name}"),
            Self::Timeout => write!(f, "job timed out"),
        }
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Failed(e) | Self::Permanent(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<String> for JobError {
    fn from(msg: String) -> Self {
        Self::Failed(msg.into())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for JobError {
    fn from(err: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Failed(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn failed_display() {
        let err = JobError::failed(std::io::Error::other("disk full"));
        assert_eq!(err.to_string(), "job failed: disk full");
    }

    #[test]
    fn permanent_display() {
        let err = JobError::permanent(std::io::Error::other("invalid input"));
        assert_eq!(err.to_string(), "job permanently failed: invalid input");
    }

    #[test]
    fn missing_state_display() {
        let err = JobError::MissingState("Database".into());
        assert_eq!(err.to_string(), "missing state: Database");
    }

    #[test]
    fn timeout_display() {
        assert_eq!(JobError::Timeout.to_string(), "job timed out");
    }

    #[test]
    fn is_permanent_for_permanent() {
        let err = JobError::permanent(std::io::Error::other("bad"));
        assert!(err.is_permanent());
    }

    #[test]
    fn is_permanent_for_missing_state() {
        assert!(JobError::MissingState("X".into()).is_permanent());
    }

    #[test]
    fn is_not_permanent_for_failed() {
        let err = JobError::failed(std::io::Error::other("retry me"));
        assert!(!err.is_permanent());
    }

    #[test]
    fn is_not_permanent_for_timeout() {
        assert!(!JobError::Timeout.is_permanent());
    }

    #[test]
    fn from_string() {
        let err: JobError = "something broke".to_string().into();
        assert!(matches!(err, JobError::Failed(_)));
        assert_eq!(err.to_string(), "job failed: something broke");
    }

    #[test]
    fn from_boxed_error() {
        let boxed: Box<dyn std::error::Error + Send + Sync> = "boxed".into();
        let err: JobError = boxed.into();
        assert!(matches!(err, JobError::Failed(_)));
    }

    #[test]
    fn error_source_for_failed() {
        let err = JobError::failed(std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn error_source_for_timeout_is_none() {
        assert!(JobError::Timeout.source().is_none());
    }
}
