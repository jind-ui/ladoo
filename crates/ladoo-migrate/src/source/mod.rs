//! Migration source abstraction and implementations.
//!
//! A [`MigrationSource`] loads migrations from some storage — the
//! filesystem (via `FilesystemSource`, added in a later task), an
//! in-memory collection (via [`InMemorySource`]), or any custom backend.
//!
//! The engine never reads files directly — it always goes through a
//! `MigrationSource`, making it trivial to test without touching disk.

pub mod parser;

use crate::migration::Migration;
use crate::MigrateError;

/// Loads migrations from some source.
///
/// The engine calls [`load_versioned`](MigrationSource::load_versioned)
/// and [`load_repeatable`](MigrationSource::load_repeatable) to discover
/// which migrations exist. The source is responsible for parsing and
/// ordering.
pub trait MigrationSource {
    /// Load all versioned migrations, ordered by version.
    fn load_versioned(&self) -> Result<Vec<Migration>, MigrateError>;

    /// Load all repeatable migrations, ordered alphabetically by filename.
    fn load_repeatable(&self) -> Result<Vec<Migration>, MigrateError>;
}

/// In-memory migration source for testing.
///
/// Pre-populated with migrations — no filesystem access needed.
///
/// # Examples
///
/// ```
/// use ladoo_migrate::source::InMemorySource;
/// use ladoo_migrate::source::MigrationSource;
///
/// let source = InMemorySource {
///     versioned: vec![],
///     repeatable: vec![],
/// };
/// assert!(source.load_versioned().unwrap().is_empty());
/// ```
pub struct InMemorySource {
    /// Versioned migrations (should be pre-sorted by version).
    pub versioned: Vec<Migration>,
    /// Repeatable migrations (should be pre-sorted alphabetically).
    pub repeatable: Vec<Migration>,
}

impl MigrationSource for InMemorySource {
    fn load_versioned(&self) -> Result<Vec<Migration>, MigrateError> {
        Ok(self.versioned.clone())
    }

    fn load_repeatable(&self) -> Result<Vec<Migration>, MigrateError> {
        Ok(self.repeatable.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_source_returns_cloned_migrations() {
        let m = Migration {
            version: "20260810_120000".into(),
            name: "create_users".into(),
            up_sql: "CREATE TABLE users (id INT);".into(),
            down_sql: None,
            down_skip_reason: None,
            checksum: "abc".into(),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        };
        let source = InMemorySource {
            versioned: vec![m],
            repeatable: vec![],
        };
        let loaded = source.load_versioned().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].version, "20260810_120000");
    }

    #[test]
    fn in_memory_source_returns_cloned_repeatable_migrations() {
        let m = Migration {
            version: "20260810_120000".into(),
            name: "now_utc".into(),
            up_sql: "CREATE OR REPLACE FUNCTION now_utc() ...".into(),
            down_sql: None,
            down_skip_reason: None,
            checksum: "abc".into(),
            no_transaction: false,
            requires: vec![],
            repeatable: true,
        };
        let source = InMemorySource {
            versioned: vec![],
            repeatable: vec![m],
        };
        let loaded = source.load_repeatable().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].repeatable);
    }

    #[test]
    fn in_memory_source_empty() {
        let source = InMemorySource {
            versioned: vec![],
            repeatable: vec![],
        };
        assert!(source.load_versioned().unwrap().is_empty());
        assert!(source.load_repeatable().unwrap().is_empty());
    }
}
