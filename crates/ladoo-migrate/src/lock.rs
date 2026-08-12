//! Advisory lock lifecycle management.
//!
//! The [`LockManager`] acquires and releases database-level advisory locks
//! to prevent concurrent migration runs. The lock key is derived from a
//! hash of the database name and table name to avoid cross-project
//! collisions.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::driver::MigrationDriver;
use crate::MigrateError;

/// Manages advisory lock acquisition and release.
///
/// Advisory locks prevent concurrent migration runs against the same
/// database. The lock key is derived from a hash of `(database_name,
/// table_name)` to avoid collisions across projects sharing a database
/// server.
pub struct LockManager;

impl LockManager {
    /// Compute a lock key from the database name and table name.
    ///
    /// Uses a deterministic hash to produce a consistent `i64` key.
    /// Different `(db, table)` pairs produce different keys, preventing
    /// cross-project lock collisions.
    pub fn compute_lock_key(db_name: &str, table_name: &str) -> i64 {
        let mut hasher = DefaultHasher::new();
        db_name.hash(&mut hasher);
        table_name.hash(&mut hasher);
        // Ensure positive i64 by masking the sign bit
        (hasher.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    /// Acquire the advisory lock.
    pub async fn acquire(driver: &impl MigrationDriver, key: i64) -> Result<(), MigrateError> {
        driver.advisory_lock(key).await
    }

    /// Release the advisory lock.
    pub async fn release(driver: &impl MigrationDriver, key: i64) -> Result<(), MigrateError> {
        driver.advisory_unlock(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_key_deterministic() {
        let a = LockManager::compute_lock_key("mydb", "_migrations");
        let b = LockManager::compute_lock_key("mydb", "_migrations");
        assert_eq!(a, b);
    }

    #[test]
    fn different_db_different_key() {
        let a = LockManager::compute_lock_key("db1", "_migrations");
        let b = LockManager::compute_lock_key("db2", "_migrations");
        assert_ne!(a, b);
    }

    #[test]
    fn different_table_different_key() {
        let a = LockManager::compute_lock_key("mydb", "_migrations");
        let b = LockManager::compute_lock_key("mydb", "schema_history");
        assert_ne!(a, b);
    }

    #[test]
    fn lock_key_is_positive() {
        let key = LockManager::compute_lock_key("test", "test");
        assert!(key > 0);
    }

    #[test]
    fn lock_key_empty_strings() {
        let key = LockManager::compute_lock_key("", "");
        assert!(key >= 0);
    }

    #[cfg(feature = "sqlite")]
    mod with_sqlite {
        use super::*;
        use crate::driver::sqlite::SqliteDriver;

        #[tokio::test]
        async fn acquire_and_release() {
            let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
            let key = LockManager::compute_lock_key("test", "_migrations");
            LockManager::acquire(&driver, key).await.unwrap();
            LockManager::release(&driver, key).await.unwrap();
        }
    }
}
