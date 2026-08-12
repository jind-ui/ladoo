//! Error types for the migration engine.
//!
//! [`MigrateError`] covers every failure mode: parse errors, checksum
//! mismatches, SQL failures, lock contention, and configuration problems.
//! Every variant includes an actionable message — users know what happened
//! and what to do without searching documentation.

/// All errors produced by the migration engine.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// A migration file could not be parsed.
    #[error("parse error in {file}: {message}")]
    Parse {
        /// Path or name of the file that failed to parse.
        file: String,
        /// Description of the parse failure.
        message: String,
    },

    /// An applied migration's checksum does not match the file on disk.
    #[error("checksum mismatch for {version}: expected {expected}, found {found}\n  fix: db repair --update-checksum {version}")]
    ChecksumMismatch {
        /// Version of the mismatched migration.
        version: String,
        /// Checksum stored in the database.
        expected: String,
        /// Checksum computed from the current file.
        found: String,
    },

    /// A migration is stuck in PARTIAL state, blocking all operations.
    #[error("migration {version} is in PARTIAL state — run `db repair` first")]
    PartialBlocking {
        /// Version of the partial migration.
        version: String,
    },

    /// A `@requires` dependency has not been applied yet.
    #[error("@requires dependency not met: {version} requires {dependency}")]
    DependencyNotMet {
        /// Version of the migration with the unmet dependency.
        version: String,
        /// Version of the required migration.
        dependency: String,
    },

    /// A `@no-transaction` migration was included in an `--atomic` run.
    #[error("@no-transaction migration {version} cannot be used with --atomic")]
    NoTransactionInAtomic {
        /// Version of the incompatible migration.
        version: String,
    },

    /// SQL execution failed.
    #[error("SQL execution error: {0}")]
    Sql(String),

    /// Database connection failed.
    #[error("database connection failed: {0}")]
    Connection(String),

    /// Advisory lock acquisition or release failed.
    #[error("advisory lock failed: {0}")]
    LockFailed(String),

    /// Rollback blocked because the migration has `@down(skip)`.
    #[error("rollback blocked: migration {version} has @down(skip): {reason}")]
    RollbackSkipped {
        /// Version of the migration that cannot be rolled back.
        version: String,
        /// Reason the author gave for skipping the down migration.
        reason: String,
    },

    /// A referenced migration was not found on disk.
    #[error("migration {version} not found on disk")]
    MigrationNotFound {
        /// Version that was expected but missing.
        version: String,
    },

    /// Configuration file error (invalid TOML, missing fields).
    #[error("config error: {0}")]
    Config(String),

    /// Filesystem I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_display() {
        let err = MigrateError::Parse {
            file: "001_init.sql".into(),
            message: "missing @up directive".into(),
        };
        assert_eq!(
            err.to_string(),
            "parse error in 001_init.sql: missing @up directive"
        );
    }

    #[test]
    fn checksum_mismatch_includes_fix_command() {
        let err = MigrateError::ChecksumMismatch {
            version: "20260810_120000".into(),
            expected: "abc".into(),
            found: "def".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("db repair --update-checksum 20260810_120000"));
    }

    #[test]
    fn partial_blocking_suggests_repair() {
        let err = MigrateError::PartialBlocking {
            version: "20260810_120000".into(),
        };
        assert!(err.to_string().contains("db repair"));
    }

    #[test]
    fn dependency_not_met_shows_both_versions() {
        let err = MigrateError::DependencyNotMet {
            version: "20260811_100000".into(),
            dependency: "20260810_100000".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("20260811_100000"));
        assert!(msg.contains("20260810_100000"));
    }

    #[test]
    fn no_transaction_in_atomic_names_version() {
        let err = MigrateError::NoTransactionInAtomic {
            version: "20260810_120000".into(),
        };
        assert!(err.to_string().contains("--atomic"));
    }

    #[test]
    fn rollback_skipped_shows_reason() {
        let err = MigrateError::RollbackSkipped {
            version: "20260810_120000".into(),
            reason: "cannot reverse concurrent index".into(),
        };
        assert!(err.to_string().contains("cannot reverse concurrent index"));
    }

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: MigrateError = io_err.into();
        assert!(err.to_string().contains("gone"));
    }

    #[test]
    fn sql_error_display() {
        let err = MigrateError::Sql("relation does not exist".into());
        assert!(err.to_string().contains("relation does not exist"));
    }

    #[test]
    fn connection_error_display() {
        let err = MigrateError::Connection("connection refused".into());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn lock_failed_display() {
        let err = MigrateError::LockFailed("timeout".into());
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn migration_not_found_display() {
        let err = MigrateError::MigrationNotFound {
            version: "20260810_120000".into(),
        };
        assert!(err.to_string().contains("20260810_120000"));
    }

    #[test]
    fn config_error_display() {
        let err = MigrateError::Config("missing url_env".into());
        assert!(err.to_string().contains("missing url_env"));
    }
}
