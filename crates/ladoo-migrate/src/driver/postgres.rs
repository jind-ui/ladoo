//! PostgreSQL driver for the migration engine.
//!
//! Uses `sqlx::PgPool` for connection management. Postgres supports
//! transactional DDL and advisory locks via `pg_advisory_lock()`.

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Executor, Row};

use super::{MigrationDriver, Transaction};
use crate::migration::{AppliedMigration, MigrationStatus};
use crate::MigrateError;

/// PostgreSQL implementation of [`MigrationDriver`].
///
/// Connects via `sqlx::PgPool`. Supports transactional DDL, advisory
/// locks, and `--atomic` mode.
pub struct PostgresDriver {
    pool: PgPool,
}

#[async_trait]
impl MigrationDriver for PostgresDriver {
    async fn connect(url: &str) -> Result<Self, MigrateError>
    where
        Self: Sized,
    {
        let pool = PgPoolOptions::new()
            .max_connections(2)
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
        Ok(Box::new(PgTransaction { inner: Some(tx) }))
    }

    async fn advisory_lock(&self, key: i64) -> Result<(), MigrateError> {
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| MigrateError::LockFailed(e.to_string()))?;
        Ok(())
    }

    async fn advisory_unlock(&self, key: i64) -> Result<(), MigrateError> {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| MigrateError::LockFailed(e.to_string()))?;
        Ok(())
    }

    fn supports_transactional_ddl(&self) -> bool {
        true
    }

    fn display_name(&self) -> &str {
        "PostgreSQL"
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
            let applied_at: chrono::DateTime<chrono::Utc> = row
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
                applied_at,
                applied_order: row
                    .try_get("applied_order")
                    .map_err(|e| MigrateError::Sql(e.to_string()))?,
                status: MigrationStatus::from_str(&status_str).unwrap_or(MigrationStatus::Applied),
            });
        }
        Ok(result)
    }
}

/// Active PostgreSQL transaction.
struct PgTransaction {
    inner: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
}

#[async_trait]
impl Transaction for PgTransaction {
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
#[cfg(feature = "test-postgres")]
mod tests {
    use super::*;

    fn pg_url() -> String {
        std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/ladoo_test".into())
    }

    #[tokio::test]
    async fn connect_and_execute() {
        let driver = PostgresDriver::connect(&pg_url()).await.unwrap();
        assert_eq!(driver.display_name(), "PostgreSQL");
        assert!(driver.supports_transactional_ddl());
        driver.execute("SELECT 1").await.unwrap();
    }

    #[tokio::test]
    async fn advisory_lock_lifecycle() {
        let driver = PostgresDriver::connect(&pg_url()).await.unwrap();
        driver.advisory_lock(99999).await.unwrap();
        driver.advisory_unlock(99999).await.unwrap();
    }

    #[tokio::test]
    async fn transaction_commit_and_rollback() {
        let driver = PostgresDriver::connect(&pg_url()).await.unwrap();
        driver
            .execute("CREATE TABLE IF NOT EXISTS _pg_test (id INT)")
            .await
            .unwrap();

        // Commit
        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO _pg_test (id) VALUES (1)")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // Rollback
        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO _pg_test (id) VALUES (2)")
            .await
            .unwrap();
        tx.rollback().await.unwrap();

        driver.execute("DROP TABLE _pg_test").await.unwrap();
    }
}
