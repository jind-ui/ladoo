//! Database driver abstraction.
//!
//! [`MigrationDriver`] is the core trait that database backends implement.
//! Each driver provides raw SQL capabilities — connection, execution,
//! transactions, and advisory locks. The engine handles all business
//! logic; drivers are deliberately thin (~100 LOC each).
//!
//! [`Transaction`] represents an active database transaction with
//! `execute`, `commit`, and `rollback` operations. `commit` and
//! `rollback` consume the transaction via `self: Box<Self>` to prevent
//! use-after-commit.

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "mysql")]
pub mod mysql;

use async_trait::async_trait;

use crate::migration::AppliedMigration;
use crate::MigrateError;

/// A database backend for the migration engine.
///
/// Drivers are deliberately thin — they provide raw SQL capabilities
/// and the engine handles all business logic (parsing, checksums,
/// ordering, recovery). Each driver is ~100 LOC.
///
/// # Implementing a Driver
///
/// ```rust,ignore
/// use ladoo_migrate::driver::{MigrationDriver, Transaction};
/// use ladoo_migrate::{MigrateError, AppliedMigration};
///
/// struct MyDriver { /* connection pool */ }
///
/// #[async_trait]
/// impl MigrationDriver for MyDriver {
///     async fn connect(url: &str) -> Result<Self, MigrateError> { todo!() }
///     async fn execute(&self, sql: &str) -> Result<(), MigrateError> { todo!() }
///     async fn begin(&self) -> Result<Box<dyn Transaction>, MigrateError> { todo!() }
///     async fn advisory_lock(&self, key: i64) -> Result<(), MigrateError> { todo!() }
///     async fn advisory_unlock(&self, key: i64) -> Result<(), MigrateError> { todo!() }
///     fn supports_transactional_ddl(&self) -> bool { true }
///     fn display_name(&self) -> &str { "mydb" }
///     async fn query_applied_migrations(&self, table: &str)
///         -> Result<Vec<AppliedMigration>, MigrateError> { todo!() }
/// }
/// ```
#[async_trait]
pub trait MigrationDriver: Send + Sync {
    /// Connect to the database using the given URL.
    async fn connect(url: &str) -> Result<Self, MigrateError>
    where
        Self: Sized;

    /// Execute arbitrary SQL against the database.
    async fn execute(&self, sql: &str) -> Result<(), MigrateError>;

    /// Begin a new transaction.
    async fn begin(&self) -> Result<Box<dyn Transaction>, MigrateError>;

    /// Acquire an advisory lock. Blocks until acquired.
    ///
    /// The key is derived from a hash of `(database_name, migrations_table_name)`
    /// to avoid cross-project collisions.
    async fn advisory_lock(&self, key: i64) -> Result<(), MigrateError>;

    /// Release the advisory lock.
    async fn advisory_unlock(&self, key: i64) -> Result<(), MigrateError>;

    /// Whether this database supports transactional DDL.
    ///
    /// Postgres and SQLite return `true`. MySQL returns `false`
    /// because DDL statements auto-commit.
    fn supports_transactional_ddl(&self) -> bool;

    /// Display name for logging and error messages.
    fn display_name(&self) -> &str;

    /// Query the migrations tracking table for applied migrations.
    ///
    /// The engine provides the table name; the driver executes the query
    /// and maps rows to [`AppliedMigration`] structs.
    async fn query_applied_migrations(
        &self,
        table: &str,
    ) -> Result<Vec<AppliedMigration>, MigrateError>;
}

/// An active database transaction.
///
/// `commit` and `rollback` consume the transaction via `self: Box<Self>`
/// to prevent use-after-commit/rollback.
#[async_trait]
pub trait Transaction: Send {
    /// Execute SQL within this transaction.
    async fn execute(&mut self, sql: &str) -> Result<(), MigrateError>;

    /// Commit the transaction. Consumes self.
    async fn commit(self: Box<Self>) -> Result<(), MigrateError>;

    /// Rollback the transaction. Consumes self.
    async fn rollback(self: Box<Self>) -> Result<(), MigrateError>;
}
