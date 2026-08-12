//! Migration planning — compute which migrations need to run.
//!
//! The [`MigrationPlan`] takes pending migrations from a source and
//! applied migrations from the database, then determines which
//! migrations to execute, in what order, validating dependencies
//! and constraints along the way.

use std::collections::{HashMap, HashSet};

use crate::migration::{AppliedMigration, Migration, MigrationStatus};
use crate::MigrateError;

/// Builds an execution plan from pending and applied migrations.
pub struct MigrationPlan;

impl MigrationPlan {
    /// Compute the list of migrations to apply.
    ///
    /// Filters out already-applied migrations, validates `@requires`
    /// dependencies, and optionally limits to a target version.
    ///
    /// # Errors
    ///
    /// - [`MigrateError::PartialBlocking`] if any applied migration is PARTIAL
    /// - [`MigrateError::ChecksumMismatch`] if an applied migration's checksum doesn't match disk
    /// - [`MigrateError::DependencyNotMet`] if a `@requires` dependency isn't met
    /// - [`MigrateError::NoTransactionInAtomic`] if atomic mode and a pending migration has `@no-transaction`
    pub fn build(
        pending: &[Migration],
        applied: &[AppliedMigration],
        target: Option<&str>,
        atomic: bool,
    ) -> Result<Vec<Migration>, MigrateError> {
        // Check for PARTIAL state
        for am in applied {
            if am.status == MigrationStatus::Partial {
                return Err(MigrateError::PartialBlocking {
                    version: am.version.clone(),
                });
            }
        }

        // Verify checksums for applied migrations that exist on disk
        let applied_versions: HashMap<&str, &str> = applied
            .iter()
            .map(|am| (am.version.as_str(), am.checksum.as_str()))
            .collect();

        for m in pending {
            if let Some(&stored_checksum) = applied_versions.get(m.version.as_str()) {
                if stored_checksum != m.checksum {
                    return Err(MigrateError::ChecksumMismatch {
                        version: m.version.clone(),
                        expected: stored_checksum.to_string(),
                        found: m.checksum.clone(),
                    });
                }
            }
        }

        // Filter to truly pending (not already applied)
        let mut to_apply: Vec<Migration> = pending
            .iter()
            .filter(|m| !applied_versions.contains_key(m.version.as_str()))
            .cloned()
            .collect();

        // Sort by version
        to_apply.sort_by(|a, b| a.version.cmp(&b.version));

        // Filter by target version
        if let Some(target) = target {
            to_apply.retain(|m| m.version.as_str() <= target);
        }

        // Validate @requires
        let mut will_be_applied: HashSet<&str> =
            applied.iter().map(|am| am.version.as_str()).collect();

        for m in &to_apply {
            for dep in &m.requires {
                if !will_be_applied.contains(dep.as_str()) {
                    return Err(MigrateError::DependencyNotMet {
                        version: m.version.clone(),
                        dependency: dep.clone(),
                    });
                }
            }
            will_be_applied.insert(&m.version);
        }

        // Check atomic compatibility
        if atomic {
            for m in &to_apply {
                if m.no_transaction {
                    return Err(MigrateError::NoTransactionInAtomic {
                        version: m.version.clone(),
                    });
                }
            }
        }

        Ok(to_apply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_migration(version: &str, name: &str) -> Migration {
        Migration {
            version: version.into(),
            name: name.into(),
            up_sql: format!("CREATE TABLE {name} (id INT);"),
            down_sql: Some(format!("DROP TABLE {name};")),
            down_skip_reason: None,
            checksum: crate::checksum::compute_checksum(&format!("CREATE TABLE {name} (id INT);")),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        }
    }

    fn make_applied(version: &str, name: &str, checksum: &str, order: i64) -> AppliedMigration {
        AppliedMigration {
            version: version.into(),
            name: name.into(),
            checksum: checksum.into(),
            up_sql: format!("CREATE TABLE {name} (id INT);"),
            down_sql: Some(format!("DROP TABLE {name};")),
            applied_at: Utc::now(),
            applied_order: order,
            status: MigrationStatus::Applied,
        }
    }

    #[test]
    fn empty_pending_returns_empty() {
        let result = MigrationPlan::build(&[], &[], None, false).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn all_pending_returned_when_none_applied() {
        let pending = vec![
            make_migration("20260810_100000", "first"),
            make_migration("20260810_120000", "second"),
        ];
        let result = MigrationPlan::build(&pending, &[], None, false).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].version, "20260810_100000");
        assert_eq!(result[1].version, "20260810_120000");
    }

    #[test]
    fn filters_already_applied() {
        let m1 = make_migration("20260810_100000", "first");
        let m2 = make_migration("20260810_120000", "second");
        let pending = vec![m1.clone(), m2];
        let applied = vec![make_applied("20260810_100000", "first", &m1.checksum, 1)];

        let result = MigrationPlan::build(&pending, &applied, None, false).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].version, "20260810_120000");
    }

    #[test]
    fn sorted_by_version() {
        let pending = vec![
            make_migration("20260810_120000", "second"),
            make_migration("20260810_100000", "first"),
        ];
        let result = MigrationPlan::build(&pending, &[], None, false).unwrap();
        assert_eq!(result[0].version, "20260810_100000");
        assert_eq!(result[1].version, "20260810_120000");
    }

    #[test]
    fn target_version_filters() {
        let pending = vec![
            make_migration("20260810_100000", "first"),
            make_migration("20260810_120000", "second"),
            make_migration("20260810_140000", "third"),
        ];
        let result = MigrationPlan::build(&pending, &[], Some("20260810_120000"), false).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].version, "20260810_120000");
    }

    #[test]
    fn partial_blocks() {
        let applied = vec![AppliedMigration {
            version: "20260810_100000".into(),
            name: "broken".into(),
            checksum: "abc".into(),
            up_sql: "BAD SQL".into(),
            down_sql: None,
            applied_at: Utc::now(),
            applied_order: 1,
            status: MigrationStatus::Partial,
        }];

        let err = MigrationPlan::build(&[], &applied, None, false).unwrap_err();
        assert!(err.to_string().contains("PARTIAL"));
    }

    #[test]
    fn checksum_mismatch_detected() {
        let m = make_migration("20260810_100000", "first");
        let applied = vec![make_applied(
            "20260810_100000",
            "first",
            "wrong_checksum",
            1,
        )];

        let err = MigrationPlan::build(&[m], &applied, None, false).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn requires_dependency_met() {
        let mut m2 = make_migration("20260810_120000", "second");
        m2.requires = vec!["20260810_100000".into()];

        let pending = vec![make_migration("20260810_100000", "first"), m2];
        let result = MigrationPlan::build(&pending, &[], None, false).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn requires_dependency_already_applied() {
        let m1 = make_migration("20260810_100000", "first");
        let mut m2 = make_migration("20260810_120000", "second");
        m2.requires = vec!["20260810_100000".into()];

        let applied = vec![make_applied("20260810_100000", "first", &m1.checksum, 1)];
        let result = MigrationPlan::build(&[m1, m2], &applied, None, false).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn requires_dependency_not_met() {
        let mut m = make_migration("20260810_120000", "second");
        m.requires = vec!["20260810_100000".into()];

        let err = MigrationPlan::build(&[m], &[], None, false).unwrap_err();
        assert!(err.to_string().contains("dependency not met"));
    }

    #[test]
    fn atomic_rejects_no_transaction() {
        let mut m = make_migration("20260810_100000", "first");
        m.no_transaction = true;

        let err = MigrationPlan::build(&[m], &[], None, true).unwrap_err();
        assert!(err.to_string().contains("--atomic"));
    }

    #[test]
    fn atomic_allows_normal_migrations() {
        let pending = vec![make_migration("20260810_100000", "first")];
        let result = MigrationPlan::build(&pending, &[], None, true).unwrap();
        assert_eq!(result.len(), 1);
    }
}
