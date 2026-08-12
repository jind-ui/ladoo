//! Test utilities and driver conformance suite.
//!
//! The [`driver_conformance`] module provides a test suite that verifies
//! any [`MigrationDriver`](crate::driver::MigrationDriver) implementation
//! works correctly with the migration engine. Community driver authors
//! use this to prove their driver is compatible.
//!
//! # Example
//!
//! ```rust,ignore
//! use ladoo_migrate::testing::driver_conformance;
//!
//! #[tokio::test]
//! async fn oracle_driver_conformance() {
//!     driver_conformance::run_all::<OracleDriver>("oracle://localhost/test").await;
//! }
//! ```

/// Conformance test suite for database driver implementations.
///
/// Runs the full engine test matrix against a driver to verify it
/// handles connections, transactions, advisory locks, and all migration
/// operations correctly.
pub mod driver_conformance {
    use crate::checksum::compute_checksum;
    use crate::driver::MigrationDriver;
    use crate::engine::{EngineConfig, MigrateOptions, MigrationEngine, RollbackStrategy};
    use crate::migration::Migration;
    use crate::source::InMemorySource;

    fn make_migration(version: &str, name: &str, sql: &str) -> Migration {
        Migration {
            version: version.into(),
            name: name.into(),
            up_sql: sql.into(),
            down_sql: Some(format!("DROP TABLE IF EXISTS {name};")),
            down_skip_reason: None,
            checksum: compute_checksum(sql),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        }
    }

    /// Run the full conformance suite against a driver.
    ///
    /// This tests: connection, execute, transactions, advisory locks,
    /// migrate, rollback, status, idempotency, and empty migrations.
    ///
    /// # Panics
    ///
    /// Panics (via `assert!`) if any conformance check fails.
    pub async fn run_all<D: MigrationDriver>(url: &str) {
        test_connect_and_execute::<D>(url).await;
        test_transaction_commit::<D>(url).await;
        test_transaction_rollback::<D>(url).await;
        test_advisory_lock::<D>(url).await;
        test_migrate_and_status::<D>(url).await;
        test_migrate_idempotent::<D>(url).await;
        test_rollback::<D>(url).await;
        test_empty_migration::<D>(url).await;
    }

    async fn test_connect_and_execute<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.expect("connect failed");
        driver
            .execute("CREATE TABLE IF NOT EXISTS _conformance_test (id INTEGER)")
            .await
            .expect("execute failed");
        driver
            .execute("DROP TABLE IF EXISTS _conformance_test")
            .await
            .expect("cleanup failed");
    }

    async fn test_transaction_commit<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        driver
            .execute("CREATE TABLE IF NOT EXISTS _tx_test (id INTEGER)")
            .await
            .unwrap();

        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO _tx_test (id) VALUES (1)")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        driver.execute("DROP TABLE _tx_test").await.unwrap();
    }

    async fn test_transaction_rollback<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        driver
            .execute("CREATE TABLE IF NOT EXISTS _tx_rb_test (id INTEGER)")
            .await
            .unwrap();

        let mut tx = driver.begin().await.unwrap();
        tx.execute("INSERT INTO _tx_rb_test (id) VALUES (1)")
            .await
            .unwrap();
        tx.rollback().await.unwrap();

        driver.execute("DROP TABLE _tx_rb_test").await.unwrap();
    }

    async fn test_advisory_lock<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        driver.advisory_lock(99999).await.unwrap();
        driver.advisory_unlock(99999).await.unwrap();
    }

    async fn test_migrate_and_status<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        let config = EngineConfig {
            migrations_table: "_conformance_mig".into(),
            ..EngineConfig::default()
        };
        let engine = MigrationEngine::new(driver, config);

        let source = InMemorySource {
            versioned: vec![make_migration(
                "20260810_100000",
                "_conf_tbl",
                "CREATE TABLE _conf_tbl (id INTEGER);",
            )],
            repeatable: vec![],
        };

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1, "should apply 1 migration");

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1, "status should show 1 applied");
        assert!(status.pending.is_empty(), "no pending migrations");

        // Cleanup
        engine.driver.execute("DROP TABLE IF EXISTS _conf_tbl").await.unwrap();
        engine.driver.execute("DROP TABLE IF EXISTS _conformance_mig").await.unwrap();
    }

    async fn test_migrate_idempotent<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        let config = EngineConfig {
            migrations_table: "_conformance_idem".into(),
            ..EngineConfig::default()
        };
        let engine = MigrationEngine::new(driver, config);

        let source = InMemorySource {
            versioned: vec![make_migration(
                "20260810_100000",
                "_idem_tbl",
                "CREATE TABLE _idem_tbl (id INTEGER);",
            )],
            repeatable: vec![],
        };

        engine.migrate(&source, None, MigrateOptions::default()).await.unwrap();
        let report = engine.migrate(&source, None, MigrateOptions::default()).await.unwrap();
        assert!(report.applied.is_empty(), "second migrate should be no-op");

        engine.driver.execute("DROP TABLE IF EXISTS _idem_tbl").await.unwrap();
        engine.driver.execute("DROP TABLE IF EXISTS _conformance_idem").await.unwrap();
    }

    async fn test_rollback<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        let config = EngineConfig {
            migrations_table: "_conformance_rb".into(),
            ..EngineConfig::default()
        };
        let engine = MigrationEngine::new(driver, config);

        let source = InMemorySource {
            versioned: vec![make_migration(
                "20260810_100000",
                "_rb_tbl",
                "CREATE TABLE _rb_tbl (id INTEGER);",
            )],
            repeatable: vec![],
        };

        engine.migrate(&source, None, MigrateOptions::default()).await.unwrap();
        let report = engine.rollback(RollbackStrategy::Last).await.unwrap();
        assert_eq!(report.rolled_back.len(), 1, "should rollback 1");

        let status = engine.status(&source).await.unwrap();
        assert!(status.applied.is_empty(), "nothing applied after rollback");

        engine.driver.execute("DROP TABLE IF EXISTS _conformance_rb").await.unwrap();
    }

    async fn test_empty_migration<D: MigrationDriver>(url: &str) {
        let driver = D::connect(url).await.unwrap();
        let config = EngineConfig {
            migrations_table: "_conformance_empty".into(),
            ..EngineConfig::default()
        };
        let engine = MigrationEngine::new(driver, config);

        let source = InMemorySource {
            versioned: vec![Migration {
                version: "20260810_100000".into(),
                name: "empty".into(),
                up_sql: String::new(),
                down_sql: None,
                down_skip_reason: None,
                checksum: compute_checksum(""),
                no_transaction: false,
                requires: vec![],
                repeatable: false,
            }],
            repeatable: vec![],
        };

        let report = engine.migrate(&source, None, MigrateOptions::default()).await.unwrap();
        assert_eq!(report.applied.len(), 1, "empty migration should be recorded");

        engine.driver.execute("DROP TABLE IF EXISTS _conformance_empty").await.unwrap();
    }
}
