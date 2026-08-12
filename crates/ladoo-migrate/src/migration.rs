//! Core data types for parsed and applied migrations.
//!
//! [`Migration`] represents a migration file parsed from disk (not yet
//! applied). [`AppliedMigration`] represents a row in the `_migrations`
//! table. [`MigrationStatus`] tracks whether an applied migration
//! completed successfully or is stuck in a partial state.

use chrono::{DateTime, Utc};

/// A parsed migration file (not yet applied to the database).
///
/// Created by the parser from a `.sql` file on disk. The [`checksum`](Migration::checksum)
/// field is the SHA-256 hex digest of the `@up` block content.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Version extracted from the filename (e.g., `"20260810_120000"`).
    pub version: String,
    /// Descriptive name extracted from the filename.
    pub name: String,
    /// SQL content of the `@up` block.
    pub up_sql: String,
    /// SQL content of the `@down` block, if present.
    pub down_sql: Option<String>,
    /// Reason given for `@down(skip)`, if the migration is irreversible.
    pub down_skip_reason: Option<String>,
    /// SHA-256 hex digest of `up_sql`.
    pub checksum: String,
    /// Whether the `@no-transaction` directive was set.
    pub no_transaction: bool,
    /// Versions this migration depends on via `@requires`.
    pub requires: Vec<String>,
    /// Whether the `@repeatable` directive was set.
    pub repeatable: bool,
}

/// A migration that has been applied to the database.
///
/// Stored in the `_migrations` table. Contains the full SQL content
/// (both `@up` and `@down`) so rollback works without files on disk.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    /// Version of the applied migration.
    pub version: String,
    /// Descriptive name of the migration.
    pub name: String,
    /// SHA-256 checksum stored at apply time.
    pub checksum: String,
    /// Full `@up` SQL stored for reference and repair.
    pub up_sql: String,
    /// Full `@down` SQL stored for rollback without files on disk.
    pub down_sql: Option<String>,
    /// Timestamp when the migration was applied.
    pub applied_at: DateTime<Utc>,
    /// Monotonically increasing order of application.
    pub applied_order: i64,
    /// Whether the migration completed or is stuck partial.
    pub status: MigrationStatus,
}

/// State of a migration in the `_migrations` table.
///
/// A migration is [`Applied`](MigrationStatus::Applied) after successful
/// completion. [`Partial`](MigrationStatus::Partial) means it started
/// but failed midway — typically from a `@no-transaction` migration or
/// MySQL DDL that auto-commits. A partial migration blocks all commands
/// except `db status` and `db repair`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    /// Migration completed successfully.
    Applied,
    /// Migration started but failed midway. Requires `db repair`.
    Partial,
}

impl MigrationStatus {
    /// Convert the status to its database string representation.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Applied => "applied",
            Self::Partial => "partial",
        }
    }

    /// Parse a status from its database string representation.
    ///
    /// Returns `None` for unrecognized values.
    ///
    /// Named `from_str` (not the `FromStr` trait) because parsing is
    /// infallible-by-design here — unrecognized input is `None`, not `Err`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "applied" => Some(Self::Applied),
            "partial" => Some(Self::Partial),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_status_roundtrip() {
        assert_eq!(
            MigrationStatus::from_str(MigrationStatus::Applied.as_str()),
            Some(MigrationStatus::Applied)
        );
        assert_eq!(
            MigrationStatus::from_str(MigrationStatus::Partial.as_str()),
            Some(MigrationStatus::Partial)
        );
    }

    #[test]
    fn migration_status_unknown_returns_none() {
        assert_eq!(MigrationStatus::from_str("unknown"), None);
    }

    #[test]
    fn migration_struct_is_constructable() {
        let m = Migration {
            version: "20260810_120000".into(),
            name: "create_users".into(),
            up_sql: "CREATE TABLE users (id INT);".into(),
            down_sql: Some("DROP TABLE users;".into()),
            down_skip_reason: None,
            checksum: "abc123".into(),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        };
        assert_eq!(m.version, "20260810_120000");
        assert!(!m.no_transaction);
        assert!(!m.repeatable);
    }

    #[test]
    fn applied_migration_struct_is_constructable() {
        let am = AppliedMigration {
            version: "20260810_120000".into(),
            name: "create_users".into(),
            checksum: "abc123".into(),
            up_sql: "CREATE TABLE users (id INT);".into(),
            down_sql: Some("DROP TABLE users;".into()),
            applied_at: Utc::now(),
            applied_order: 1,
            status: MigrationStatus::Applied,
        };
        assert_eq!(am.applied_order, 1);
        assert_eq!(am.status, MigrationStatus::Applied);
    }

    #[test]
    fn migration_with_down_skip() {
        let m = Migration {
            version: "20260810_120000".into(),
            name: "add_index".into(),
            up_sql: "CREATE INDEX CONCURRENTLY idx ON t(col);".into(),
            down_sql: None,
            down_skip_reason: Some("cannot reverse concurrent index".into()),
            checksum: "def456".into(),
            no_transaction: true,
            requires: vec![],
            repeatable: false,
        };
        assert!(m.down_sql.is_none());
        assert_eq!(
            m.down_skip_reason.as_deref(),
            Some("cannot reverse concurrent index")
        );
        assert!(m.no_transaction);
    }

    #[test]
    fn migration_with_requires() {
        let m = Migration {
            version: "20260811_100000".into(),
            name: "add_orders".into(),
            up_sql: "CREATE TABLE orders (id INT);".into(),
            down_sql: Some("DROP TABLE orders;".into()),
            down_skip_reason: None,
            checksum: "ghi789".into(),
            no_transaction: false,
            requires: vec!["20260810_120000".into()],
            repeatable: false,
        };
        assert_eq!(m.requires, vec!["20260810_120000"]);
    }
}
