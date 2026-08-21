//! Error types for the job queue.

/// Errors produced by job store operations.
#[derive(Debug, thiserror::Error)]
pub enum JobStoreError {
    /// Database operation failed.
    #[error("database error: {0}")]
    Database(String),

    /// Job payload serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Job with the given ID was not found.
    #[error("job {0} not found")]
    NotFound(crate::store::JobId),

    /// Job is already locked by another worker.
    #[error("job {0} is already locked")]
    AlreadyLocked(crate::store::JobId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_error_display() {
        let err = JobStoreError::Database("connection refused".into());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn serialization_error_display() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = JobStoreError::Serialization(json_err);
        assert!(err.to_string().contains("serialization"));
    }

    #[test]
    fn not_found_display() {
        let err = JobStoreError::NotFound(42);
        assert_eq!(err.to_string(), "job 42 not found");
    }

    #[test]
    fn already_locked_display() {
        let err = JobStoreError::AlreadyLocked(7);
        assert_eq!(err.to_string(), "job 7 is already locked");
    }
}
