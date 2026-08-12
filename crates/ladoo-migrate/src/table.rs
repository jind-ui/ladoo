//! Migrations tracking table management.
//!
//! The [`TableManager`] handles the `_migrations` table DDL and all
//! CRUD operations: creating the table, recording applied migrations,
//! tracking PARTIAL state, and cleanup on rollback. All SQL is standard
//! and works across Postgres, MySQL, and SQLite.

use crate::driver::MigrationDriver;
use crate::migration::{Migration, MigrationStatus};
use crate::MigrateError;

/// Manages the `_migrations` tracking table.
///
/// All operations are expressed as standard SQL executed through the
/// [`MigrationDriver`]. The table stores the full `@up` and `@down`
/// SQL so rollback works without files on disk.
pub struct TableManager;

impl TableManager {
    /// Create the migrations table if it does not exist.
    pub async fn ensure_table(
        driver: &impl MigrationDriver,
        table: &str,
    ) -> Result<(), MigrateError> {
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                version        TEXT PRIMARY KEY,
                name           TEXT NOT NULL,
                checksum       TEXT NOT NULL,
                up_sql         TEXT NOT NULL,
                down_sql       TEXT,
                applied_at     TEXT NOT NULL,
                applied_order  INTEGER NOT NULL,
                status         TEXT NOT NULL DEFAULT 'applied'
            )"
        );
        driver.execute(&sql).await
    }

    /// Record a successfully applied migration.
    pub async fn record_migration(
        driver: &impl MigrationDriver,
        table: &str,
        migration: &Migration,
        applied_order: i64,
    ) -> Result<(), MigrateError> {
        Self::insert_row(
            driver,
            table,
            migration,
            applied_order,
            MigrationStatus::Applied,
        )
        .await
    }

    /// Record a migration in PARTIAL state (started but failed).
    pub async fn record_partial(
        driver: &impl MigrationDriver,
        table: &str,
        migration: &Migration,
        applied_order: i64,
    ) -> Result<(), MigrateError> {
        Self::insert_row(
            driver,
            table,
            migration,
            applied_order,
            MigrationStatus::Partial,
        )
        .await
    }

    async fn insert_row(
        driver: &impl MigrationDriver,
        table: &str,
        migration: &Migration,
        applied_order: i64,
        status: MigrationStatus,
    ) -> Result<(), MigrateError> {
        let applied_at = chrono::Utc::now().to_rfc3339();
        let down_sql_escaped = migration.down_sql.as_deref().map(|s| s.replace('\'', "''"));
        let down_sql_value = match &down_sql_escaped {
            Some(s) => format!("'{s}'"),
            None => "NULL".to_string(),
        };
        let sql = format!(
            "INSERT INTO {table} (version, name, checksum, up_sql, down_sql, applied_at, applied_order, status) \
             VALUES ('{}', '{}', '{}', '{}', {}, '{}', {}, '{}')",
            migration.version,
            migration.name.replace('\'', "''"),
            migration.checksum,
            migration.up_sql.replace('\'', "''"),
            down_sql_value,
            applied_at,
            applied_order,
            status.as_str(),
        );
        driver.execute(&sql).await
    }

    /// Update the status of an applied migration.
    pub async fn update_status(
        driver: &impl MigrationDriver,
        table: &str,
        version: &str,
        status: MigrationStatus,
    ) -> Result<(), MigrateError> {
        let sql = format!(
            "UPDATE {table} SET status = '{}' WHERE version = '{version}'",
            status.as_str(),
        );
        driver.execute(&sql).await
    }

    /// Delete a migration record (used during rollback).
    pub async fn delete_migration(
        driver: &impl MigrationDriver,
        table: &str,
        version: &str,
    ) -> Result<(), MigrateError> {
        let sql = format!("DELETE FROM {table} WHERE version = '{version}'");
        driver.execute(&sql).await
    }

    /// Get the next applied_order value (max + 1, or 1 if table is empty).
    pub async fn next_order(
        driver: &impl MigrationDriver,
        table: &str,
    ) -> Result<i64, MigrateError> {
        // This is called after ensure_table, so the table exists.
        // We query via the driver's query_applied_migrations to get the max order.
        let applied = driver.query_applied_migrations(table).await?;
        Ok(applied.iter().map(|m| m.applied_order).max().unwrap_or(0) + 1)
    }

    /// Update the stored checksum for a migration (used by `db repair --update-checksum`).
    pub async fn update_checksum(
        driver: &impl MigrationDriver,
        table: &str,
        version: &str,
        checksum: &str,
    ) -> Result<(), MigrateError> {
        let sql = format!("UPDATE {table} SET checksum = '{checksum}' WHERE version = '{version}'");
        driver.execute(&sql).await
    }
}

#[cfg(test)]
mod tests {
    // Tests require SQLite — run with `cargo test --features sqlite`
    #[cfg(feature = "sqlite")]
    mod with_sqlite {
        use super::super::*;
        use crate::driver::sqlite::SqliteDriver;

        async fn setup() -> SqliteDriver {
            SqliteDriver::connect("sqlite::memory:").await.unwrap()
        }

        #[tokio::test]
        async fn ensure_table_creates_table() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();
            // Verify by querying
            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert!(applied.is_empty());
        }

        #[tokio::test]
        async fn ensure_table_idempotent() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn record_and_query_migration() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let migration = Migration {
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

            TableManager::record_migration(&driver, "_migrations", &migration, 1)
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied.len(), 1);
            assert_eq!(applied[0].version, "20260810_120000");
            assert_eq!(applied[0].name, "create_users");
            assert_eq!(applied[0].status, MigrationStatus::Applied);
            assert_eq!(applied[0].down_sql.as_deref(), Some("DROP TABLE users;"));
        }

        #[tokio::test]
        async fn record_migration_with_no_down_sql() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let migration = Migration {
                version: "20260810_130000".into(),
                name: "irreversible".into(),
                up_sql: "CREATE INDEX CONCURRENTLY idx ON t(col);".into(),
                down_sql: None,
                down_skip_reason: Some("cannot reverse".into()),
                checksum: "abc123".into(),
                no_transaction: true,
                requires: vec![],
                repeatable: false,
            };

            TableManager::record_migration(&driver, "_migrations", &migration, 1)
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied.len(), 1);
            assert!(applied[0].down_sql.is_none());
        }

        #[tokio::test]
        async fn record_partial_sets_status() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let migration = Migration {
                version: "20260810_120000".into(),
                name: "create_users".into(),
                up_sql: "CREATE TABLE users (id INT);".into(),
                down_sql: None,
                down_skip_reason: None,
                checksum: "abc123".into(),
                no_transaction: true,
                requires: vec![],
                repeatable: false,
            };

            TableManager::record_partial(&driver, "_migrations", &migration, 1)
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied[0].status, MigrationStatus::Partial);
        }

        #[tokio::test]
        async fn update_status() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let migration = Migration {
                version: "20260810_120000".into(),
                name: "test".into(),
                up_sql: "SELECT 1;".into(),
                down_sql: None,
                down_skip_reason: None,
                checksum: "abc".into(),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            };
            TableManager::record_partial(&driver, "_migrations", &migration, 1)
                .await
                .unwrap();

            TableManager::update_status(
                &driver,
                "_migrations",
                "20260810_120000",
                MigrationStatus::Applied,
            )
            .await
            .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied[0].status, MigrationStatus::Applied);
        }

        #[tokio::test]
        async fn delete_migration() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let migration = Migration {
                version: "20260810_120000".into(),
                name: "test".into(),
                up_sql: "SELECT 1;".into(),
                down_sql: None,
                down_skip_reason: None,
                checksum: "abc".into(),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            };
            TableManager::record_migration(&driver, "_migrations", &migration, 1)
                .await
                .unwrap();

            TableManager::delete_migration(&driver, "_migrations", "20260810_120000")
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert!(applied.is_empty());
        }

        #[tokio::test]
        async fn next_order_empty_table() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let order = TableManager::next_order(&driver, "_migrations")
                .await
                .unwrap();
            assert_eq!(order, 1);
        }

        #[tokio::test]
        async fn next_order_after_inserts() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let m = Migration {
                version: "20260810_120000".into(),
                name: "test".into(),
                up_sql: "SELECT 1;".into(),
                down_sql: None,
                down_skip_reason: None,
                checksum: "abc".into(),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            };
            TableManager::record_migration(&driver, "_migrations", &m, 1)
                .await
                .unwrap();

            let order = TableManager::next_order(&driver, "_migrations")
                .await
                .unwrap();
            assert_eq!(order, 2);
        }

        #[tokio::test]
        async fn update_checksum() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let m = Migration {
                version: "20260810_120000".into(),
                name: "test".into(),
                up_sql: "SELECT 1;".into(),
                down_sql: None,
                down_skip_reason: None,
                checksum: "old_hash".into(),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            };
            TableManager::record_migration(&driver, "_migrations", &m, 1)
                .await
                .unwrap();

            TableManager::update_checksum(&driver, "_migrations", "20260810_120000", "new_hash")
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied[0].checksum, "new_hash");
        }

        #[tokio::test]
        async fn record_migration_with_quotes_in_sql() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();

            let m = Migration {
                version: "20260810_120000".into(),
                name: "insert_data".into(),
                up_sql: "INSERT INTO t VALUES ('it''s');".into(),
                down_sql: Some("DELETE FROM t WHERE v = 'it''s';".into()),
                down_skip_reason: None,
                checksum: "abc".into(),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            };
            TableManager::record_migration(&driver, "_migrations", &m, 1)
                .await
                .unwrap();

            let applied = driver
                .query_applied_migrations("_migrations")
                .await
                .unwrap();
            assert_eq!(applied.len(), 1);
            assert_eq!(applied[0].up_sql, "INSERT INTO t VALUES ('it''s');");
            assert_eq!(
                applied[0].down_sql.as_deref(),
                Some("DELETE FROM t WHERE v = 'it''s';")
            );
        }

        #[tokio::test]
        async fn ensure_table_uses_custom_table_name() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "schema_history")
                .await
                .unwrap();
            let applied = driver
                .query_applied_migrations("schema_history")
                .await
                .unwrap();
            assert!(applied.is_empty());
        }

        #[tokio::test]
        async fn delete_missing_migration_is_a_noop() {
            let driver = setup().await;
            TableManager::ensure_table(&driver, "_migrations")
                .await
                .unwrap();
            TableManager::delete_migration(&driver, "_migrations", "does_not_exist")
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn ensure_table_missing_table_error_propagates() {
            let driver = setup().await;
            let err = TableManager::next_order(&driver, "_no_such_table")
                .await
                .unwrap_err();
            assert!(matches!(err, MigrateError::Sql(_)));
        }
    }
}
