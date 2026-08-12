//! MySQL driver for the migration engine.
//!
//! Uses `sqlx::MySqlPool` for connection management. MySQL does NOT
//! support transactional DDL (DDL statements auto-commit), so
//! `@no-transaction` migrations are the norm for DDL. Advisory locks
//! use `GET_LOCK()` / `RELEASE_LOCK()`.

use async_trait::async_trait;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::{Executor, Row};

use super::{MigrationDriver, Transaction};
use crate::migration::{AppliedMigration, MigrationStatus};
use crate::MigrateError;

/// MySQL implementation of [`MigrationDriver`].
///
/// Connects via `sqlx::MySqlPool`. Does NOT support transactional DDL
/// — `supports_transactional_ddl()` returns `false`. Advisory locks
/// use `GET_LOCK()` with a 30-second timeout.
pub struct MysqlDriver {
    pool: MySqlPool,
}

#[async_trait]
impl MigrationDriver for MysqlDriver {
    async fn connect(url: &str) -> Result<Self, MigrateError>
    where
        Self: Sized,
    {
        let pool = MySqlPoolOptions::new()
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
        Ok(Box::new(MysqlTransaction { inner: Some(tx) }))
    }

    async fn advisory_lock(&self, key: i64) -> Result<(), MigrateError> {
        let lock_name = format!("ladoo_migrate_{key}");
        let sql = format!("SELECT GET_LOCK('{lock_name}', 30)");
        let row = sqlx::query(&sql)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MigrateError::LockFailed(e.to_string()))?;
        let result: i32 = row
            .try_get(0)
            .map_err(|e| MigrateError::LockFailed(e.to_string()))?;
        if result != 1 {
            return Err(MigrateError::LockFailed(
                "could not acquire advisory lock (timeout or error)".into(),
            ));
        }
        Ok(())
    }

    async fn advisory_unlock(&self, key: i64) -> Result<(), MigrateError> {
        let lock_name = format!("ladoo_migrate_{key}");
        let sql = format!("SELECT RELEASE_LOCK('{lock_name}')");
        self.pool
            .execute(sql.as_str())
            .await
            .map_err(|e| MigrateError::LockFailed(e.to_string()))?;
        Ok(())
    }

    fn supports_transactional_ddl(&self) -> bool {
        false
    }

    fn display_name(&self) -> &str {
        "MySQL"
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

/// Active MySQL transaction.
struct MysqlTransaction {
    inner: Option<sqlx::Transaction<'static, sqlx::MySql>>,
}

#[async_trait]
impl Transaction for MysqlTransaction {
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
#[cfg(feature = "test-mysql")]
mod tests {
    use super::*;

    fn mysql_url() -> String {
        std::env::var("TEST_MYSQL_URL")
            .unwrap_or_else(|_| "mysql://root:root@localhost/ladoo_test".into())
    }

    #[tokio::test]
    async fn connect_and_execute() {
        let driver = MysqlDriver::connect(&mysql_url()).await.unwrap();
        assert_eq!(driver.display_name(), "MySQL");
        assert!(!driver.supports_transactional_ddl());
        driver.execute("SELECT 1").await.unwrap();
    }

    #[tokio::test]
    async fn advisory_lock_lifecycle() {
        let driver = MysqlDriver::connect(&mysql_url()).await.unwrap();
        driver.advisory_lock(99999).await.unwrap();
        driver.advisory_unlock(99999).await.unwrap();
    }
}
