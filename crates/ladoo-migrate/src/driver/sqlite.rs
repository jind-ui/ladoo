//! SQLite driver for the migration engine.
//!
//! Uses `sqlx::SqlitePool` for connection management. SQLite supports
//! transactional DDL, so `--atomic` mode works. Advisory locking uses
//! a no-op implementation since SQLite's file-level locking naturally
//! serializes access.

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::{Executor, Row};

use super::{MigrationDriver, Transaction};
use crate::migration::{AppliedMigration, MigrationStatus};
use crate::MigrateError;

/// SQLite implementation of [`MigrationDriver`].
///
/// Connects via `sqlx::SqlitePool`. Supports transactional DDL and
/// `--atomic` mode. Advisory locks are no-ops since SQLite's file lock
/// serializes writes.
#[derive(Debug)]
pub struct SqliteDriver {
    pool: SqlitePool,
}

#[async_trait]
impl MigrationDriver for SqliteDriver {
    async fn connect(url: &str) -> Result<Self, MigrateError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|e| MigrateError::Connection(e.to_string()))?;
        Ok(Self { pool })
    }

    async fn execute(&self, sql: &str) -> Result<(), MigrateError> {
        self.pool
            .execute(sql)
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;
        Ok(())
    }

    async fn begin(&self) -> Result<Box<dyn Transaction>, MigrateError> {
        let tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;
        Ok(Box::new(SqliteTransaction { inner: Some(tx) }))
    }

    async fn advisory_lock(&self, _key: i64) -> Result<(), MigrateError> {
        // SQLite uses file-level locking — no advisory lock needed.
        Ok(())
    }

    async fn advisory_unlock(&self, _key: i64) -> Result<(), MigrateError> {
        Ok(())
    }

    fn supports_transactional_ddl(&self) -> bool {
        true
    }

    fn display_name(&self) -> &str {
        "SQLite"
    }

    async fn query_applied_migrations(
        &self,
        table: &str,
    ) -> Result<Vec<AppliedMigration>, MigrateError> {
        let sql = format!(
            "SELECT version, name, checksum, up_sql, down_sql, \
             applied_at, applied_order, status \
             FROM {table} ORDER BY applied_order"
        );

        let rows = sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let status_str: String = row
                .try_get("status")
                .map_err(|e| MigrateError::Sql(e.to_string()))?;
            let applied_at_str: String = row
                .try_get("applied_at")
                .map_err(|e| MigrateError::Sql(e.to_string()))?;

            result.push(AppliedMigration {
                version: row
                    .try_get("version")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                name: row
                    .try_get("name")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                checksum: row
                    .try_get("checksum")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                up_sql: row
                    .try_get("up_sql")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                down_sql: row
                    .try_get("down_sql")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                applied_at: chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                applied_order: row
                    .try_get("applied_order")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                status: MigrationStatus::from_str(&status_str).unwrap_or(MigrationStatus::Applied),
            });
        }
        Ok(result)
    }
}

/// Active SQLite transaction.
struct SqliteTransaction {
    inner: Option<sqlx::Transaction<'static, sqlx::Sqlite>>,
}

#[async_trait]
impl Transaction for SqliteTransaction {
    async fn execute(&mut self, sql: &str) -> Result<(), MigrateError> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| MigrateError::Sql("transaction already consumed".into()))?;
        tx.execute(sql)
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;
        Ok(())
    }

    async fn commit(mut self: Box<Self>) -> Result<(), MigrateError> {
        let tx = self
            .inner
            .take()
            .ok_or_else(|| MigrateError::Sql("transaction already consumed".into()))?;
        tx.commit()
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;
        Ok(())
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), MigrateError> {
        let tx = self
            .inner
            .take()
            .ok_or_else(|| MigrateError::Sql("transaction already consumed".into()))?;
        tx.rollback()
            .await
            .map_err(|e| MigrateError::Sql(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_in_memory() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        assert_eq!(driver.display_name(), "SQLite");
        assert!(driver.supports_transactional_ddl());
    }

    #[tokio::test]
    async fn execute_sql() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute("CREATE TABLE test (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        driver
            .execute("INSERT INTO test (id) VALUES (1)")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn execute_invalid_sql_returns_error() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let err = driver.execute("INVALID SQL STATEMENT").await.unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));
    }

    #[tokio::test]
    async fn transaction_commit() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute("CREATE TABLE test (id INTEGER)")
            .await
            .unwrap();

        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO test (id) VALUES (42)").await.unwrap();
        tx.commit().await.unwrap();

        let rows = sqlx::query("SELECT id FROM test")
            .fetch_all(&driver.pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn transaction_rollback() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute("CREATE TABLE test (id INTEGER)")
            .await
            .unwrap();

        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO test (id) VALUES (42)").await.unwrap();
        tx.rollback().await.unwrap();

        let rows = sqlx::query("SELECT id FROM test")
            .fetch_all(&driver.pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 0);
    }

    #[tokio::test]
    async fn execute_after_commit_fails() {
        let mut tx = SqliteTransaction { inner: None };
        let err = tx
            .execute("INSERT INTO test (id) VALUES (1)")
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));
    }

    #[tokio::test]
    async fn commit_already_consumed_fails() {
        let tx: Box<dyn Transaction> = Box::new(SqliteTransaction { inner: None });
        let err = tx.commit().await.unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));
    }

    #[tokio::test]
    async fn rollback_already_consumed_fails() {
        let tx: Box<dyn Transaction> = Box::new(SqliteTransaction { inner: None });
        let err = tx.rollback().await.unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));
    }

    #[tokio::test]
    async fn advisory_lock_noop() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver.advisory_lock(12345).await.unwrap();
        driver.advisory_unlock(12345).await.unwrap();
    }

    #[tokio::test]
    async fn query_applied_empty() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute(
                "CREATE TABLE _migrations (
                    version TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    up_sql TEXT NOT NULL,
                    down_sql TEXT,
                    applied_at TEXT NOT NULL,
                    applied_order INTEGER NOT NULL,
                    status TEXT NOT NULL DEFAULT 'applied'
                )",
            )
            .await
            .unwrap();

        let applied = driver.query_applied_migrations("_migrations").await.unwrap();
        assert!(applied.is_empty());
    }

    #[tokio::test]
    async fn query_applied_returns_rows() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute(
                "CREATE TABLE _migrations (
                    version TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    up_sql TEXT NOT NULL,
                    down_sql TEXT,
                    applied_at TEXT NOT NULL,
                    applied_order INTEGER NOT NULL,
                    status TEXT NOT NULL DEFAULT 'applied'
                )",
            )
            .await
            .unwrap();
        driver
            .execute(
                "INSERT INTO _migrations (version, name, checksum, up_sql, down_sql, applied_at, applied_order, status) \
                 VALUES ('20260810_120000', 'create_users', 'abc123', 'CREATE TABLE users (id INT);', 'DROP TABLE users;', \
                 '2026-08-10T12:00:00+00:00', 1, 'applied')",
            )
            .await
            .unwrap();

        let applied = driver.query_applied_migrations("_migrations").await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].version, "20260810_120000");
        assert_eq!(applied[0].name, "create_users");
        assert_eq!(applied[0].status, MigrationStatus::Applied);
        assert_eq!(applied[0].down_sql.as_deref(), Some("DROP TABLE users;"));
    }

    #[tokio::test]
    async fn query_applied_unknown_status_defaults_to_applied() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        driver
            .execute(
                "CREATE TABLE _migrations (
                    version TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    up_sql TEXT NOT NULL,
                    down_sql TEXT,
                    applied_at TEXT NOT NULL,
                    applied_order INTEGER NOT NULL,
                    status TEXT NOT NULL DEFAULT 'applied'
                )",
            )
            .await
            .unwrap();
        driver
            .execute(
                "INSERT INTO _migrations (version, name, checksum, up_sql, down_sql, applied_at, applied_order, status) \
                 VALUES ('20260810_120000', 'create_users', 'abc123', 'CREATE TABLE users (id INT);', NULL, \
                 'not-a-date', 1, 'bogus')",
            )
            .await
            .unwrap();

        let applied = driver.query_applied_migrations("_migrations").await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].status, MigrationStatus::Applied);
        assert!(applied[0].down_sql.is_none());
    }

    #[tokio::test]
    async fn query_applied_missing_table_returns_error() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let err = driver
            .query_applied_migrations("_missing_table")
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));
    }

    #[tokio::test]
    async fn connect_invalid_url_returns_error() {
        let err = SqliteDriver::connect("sqlite:///nonexistent/path/db.sqlite3")
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Connection(_)));
    }
}
