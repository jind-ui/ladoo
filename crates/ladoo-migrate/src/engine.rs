//! The migration engine — orchestrates all migration operations.
//!
//! [`MigrationEngine`] is the central coordinator. It owns a
//! [`MigrationDriver`] and an [`EngineConfig`], and exposes high-level
//! operations: [`migrate`](MigrationEngine::migrate),
//! [`status`](MigrationEngine::status),
//! [`rollback`](MigrationEngine::rollback),
//! [`repair`](MigrationEngine::repair), and
//! [`baseline`](MigrationEngine::baseline).
//!
//! The engine handles all business logic — drivers provide only raw SQL.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::brand;
use crate::driver::{MigrationDriver, Transaction};
use crate::lock::LockManager;
use crate::migration::{AppliedMigration, Migration, MigrationStatus};
use crate::plan::MigrationPlan;
use crate::source::MigrationSource;
use crate::table::TableManager;
use crate::MigrateError;

/// Configuration for the migration engine.
///
/// All fields have sensible defaults via [`Default`]. Override only
/// what you need.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Name of the migrations tracking table. Default: `"_migrations"`.
    pub migrations_table: String,
    /// Directory containing migration files. Default: `"migrations/"`.
    pub migrations_dir: PathBuf,
    /// Advisory lock key. If `None`, derived from a hash of the
    /// database name and table name.
    pub lock_key: Option<i64>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            migrations_table: brand::DEFAULT_TABLE.to_string(),
            migrations_dir: PathBuf::from(brand::DEFAULT_DIR),
            lock_key: None,
        }
    }
}

/// Options for the `migrate` operation.
#[derive(Debug, Clone, Default)]
pub struct MigrateOptions {
    /// If `true`, show what would be applied without executing.
    pub dry_run: bool,
    /// If `true`, run all pending migrations in a single transaction.
    pub atomic: bool,
}

/// Report from a successful `migrate` operation.
#[derive(Debug)]
pub struct MigrateReport {
    /// Migrations that were applied (or that would be applied, for dry runs).
    pub applied: Vec<MigratedEntry>,
    /// Total wall-clock time.
    pub elapsed: Duration,
}

/// A single migration that was applied.
#[derive(Debug)]
pub struct MigratedEntry {
    /// Version of the applied migration.
    pub version: String,
    /// Descriptive name.
    pub name: String,
    /// Time taken to execute this migration.
    pub elapsed: Duration,
}

/// Report from the `status` operation.
#[derive(Debug)]
pub struct StatusReport {
    /// Migrations already applied to the database.
    pub applied: Vec<AppliedMigration>,
    /// Migrations on disk that haven't been applied yet.
    pub pending: Vec<Migration>,
    /// Repeatable migrations whose checksum has changed since last applied.
    pub repeatable_changed: Vec<Migration>,
}

/// Strategy for the [`rollback`](MigrationEngine::rollback) operation.
#[derive(Debug, Clone)]
pub enum RollbackStrategy {
    /// Rollback the most recently applied migration.
    Last,
    /// Rollback the last N migrations.
    Steps(usize),
    /// Rollback all migrations applied after this version.
    ToVersion(String),
}

/// Report from a [`rollback`](MigrationEngine::rollback) operation.
#[derive(Debug)]
pub struct RollbackReport {
    /// Versions that were rolled back, in the order they were rolled back.
    pub rolled_back: Vec<String>,
    /// Total wall-clock time.
    pub elapsed: Duration,
}

/// Strategy for the [`repair`](MigrationEngine::repair) operation.
#[derive(Debug, Clone)]
pub enum RepairStrategy {
    /// Re-run the PARTIAL migration's `@up` SQL.
    Retry,
    /// Rollback the PARTIAL migration using its stored `@down` SQL.
    Rollback,
    /// Mark the PARTIAL migration as applied (last resort).
    Skip,
    /// Update the stored checksum for a migration after an intentional file edit.
    UpdateChecksum(String),
}

/// Report from a [`repair`](MigrationEngine::repair) operation.
#[derive(Debug)]
pub struct RepairReport {
    /// Description of the action taken.
    pub action: String,
    /// Version of the repaired migration.
    pub version: String,
    /// Whether the repair succeeded.
    pub success: bool,
}

/// Build the `INSERT` statement used to record a migration inside an
/// already-open transaction.
///
/// [`TableManager`] offers the equivalent for direct (non-transactional)
/// execution against a [`MigrationDriver`], but transactions only expose
/// [`Transaction::execute`], so the engine builds the same statement here
/// for use inside a transaction.
fn insert_sql(
    table: &str,
    migration: &Migration,
    applied_order: i64,
    status: MigrationStatus,
    applied_at: &str,
) -> String {
    let down_sql_escaped = migration.down_sql.as_deref().map(|s| s.replace('\'', "''"));
    let down_sql_value = match &down_sql_escaped {
        Some(s) => format!("'{s}'"),
        None => "NULL".to_string(),
    };
    format!(
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
    )
}

/// The migration engine — orchestrates all operations.
///
/// Generic over the database driver. Create with [`new`](MigrationEngine::new)
/// and call [`migrate`](MigrationEngine::migrate) or
/// [`status`](MigrationEngine::status).
pub struct MigrationEngine<D: MigrationDriver> {
    driver: D,
    config: EngineConfig,
}

impl<D: MigrationDriver> MigrationEngine<D> {
    /// Create a new engine with the given driver and configuration.
    pub fn new(driver: D, config: EngineConfig) -> Self {
        Self { driver, config }
    }

    /// Returns a reference to the engine's configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    fn lock_key(&self) -> i64 {
        self.config.lock_key.unwrap_or_else(|| {
            LockManager::compute_lock_key(self.driver.display_name(), &self.config.migrations_table)
        })
    }

    /// Apply all pending migrations (or up to a target version).
    ///
    /// Follows the algorithm in the spec: acquire lock, ensure table,
    /// build plan, execute each migration, handle repeatables, release
    /// lock. The lock is always released, even if execution fails —
    /// if both execution and release fail, the execution error is
    /// returned since it is the more actionable failure.
    pub async fn migrate(
        &self,
        source: &impl MigrationSource,
        target: Option<&str>,
        opts: MigrateOptions,
    ) -> Result<MigrateReport, MigrateError> {
        let total_start = Instant::now();
        let lock_key = self.lock_key();
        let table = self.config.migrations_table.clone();

        LockManager::acquire(&self.driver, lock_key).await?;

        let result = self.migrate_inner(source, target, &opts, &table).await;

        let release_result = LockManager::release(&self.driver, lock_key).await;

        let mut report = result?;
        release_result?;
        report.elapsed = total_start.elapsed();
        Ok(report)
    }

    async fn migrate_inner(
        &self,
        source: &impl MigrationSource,
        target: Option<&str>,
        opts: &MigrateOptions,
        table: &str,
    ) -> Result<MigrateReport, MigrateError> {
        TableManager::ensure_table(&self.driver, table).await?;

        let applied = self.driver.query_applied_migrations(table).await?;
        let pending = source.load_versioned()?;

        let to_apply = MigrationPlan::build(&pending, &applied, target, opts.atomic)?;

        if opts.dry_run {
            let entries = to_apply
                .iter()
                .map(|m| MigratedEntry {
                    version: m.version.clone(),
                    name: m.name.clone(),
                    elapsed: Duration::ZERO,
                })
                .collect();
            return Ok(MigrateReport {
                applied: entries,
                elapsed: Duration::ZERO,
            });
        }

        let mut entries = Vec::new();
        let mut order = TableManager::next_order(&self.driver, table).await?;

        if opts.atomic {
            order = self
                .execute_atomic(table, &to_apply, order, &mut entries)
                .await?;
        } else {
            order = self
                .execute_sequential(table, &to_apply, order, &mut entries)
                .await?;
        }

        self.apply_repeatables(source, table, order, &mut entries)
            .await?;

        Ok(MigrateReport {
            applied: entries,
            elapsed: Duration::ZERO,
        })
    }

    /// Run every migration in `to_apply` inside a single transaction.
    async fn execute_atomic(
        &self,
        table: &str,
        to_apply: &[Migration],
        mut order: i64,
        entries: &mut Vec<MigratedEntry>,
    ) -> Result<i64, MigrateError> {
        let mut tx = self.driver.begin().await?;

        for m in to_apply {
            let start = Instant::now();
            if let Err(e) = self.run_and_record(tx.as_mut(), table, m, order).await {
                tx.rollback().await?;
                return Err(e);
            }
            entries.push(MigratedEntry {
                version: m.version.clone(),
                name: m.name.clone(),
                elapsed: start.elapsed(),
            });
            order += 1;
        }

        tx.commit().await?;
        Ok(order)
    }

    /// Run each migration in `to_apply` in its own transaction (or,
    /// for `@no-transaction` migrations, directly against the driver).
    async fn execute_sequential(
        &self,
        table: &str,
        to_apply: &[Migration],
        mut order: i64,
        entries: &mut Vec<MigratedEntry>,
    ) -> Result<i64, MigrateError> {
        for m in to_apply {
            let start = Instant::now();

            if m.no_transaction {
                self.run_no_transaction(table, m, order).await?;
            } else {
                let mut tx = self.driver.begin().await?;
                match self.run_and_record(tx.as_mut(), table, m, order).await {
                    Ok(()) => tx.commit().await?,
                    Err(e) => {
                        tx.rollback().await?;
                        return Err(e);
                    }
                }
            }

            entries.push(MigratedEntry {
                version: m.version.clone(),
                name: m.name.clone(),
                elapsed: start.elapsed(),
            });
            order += 1;
        }
        Ok(order)
    }

    /// Execute a migration's `@up` SQL and record it, both within `tx`.
    async fn run_and_record(
        &self,
        tx: &mut dyn Transaction,
        table: &str,
        m: &Migration,
        order: i64,
    ) -> Result<(), MigrateError> {
        tx.execute(&m.up_sql).await?;
        let applied_at = chrono::Utc::now().to_rfc3339();
        let sql = insert_sql(table, m, order, MigrationStatus::Applied, &applied_at);
        tx.execute(&sql).await
    }

    /// Run a `@no-transaction` migration directly against the driver.
    ///
    /// Recorded as PARTIAL before execution starts; if the SQL fails
    /// midway, the row is left as PARTIAL and `db repair` is required
    /// to unblock further migrations.
    async fn run_no_transaction(
        &self,
        table: &str,
        m: &Migration,
        order: i64,
    ) -> Result<(), MigrateError> {
        TableManager::record_partial(&self.driver, table, m, order).await?;
        self.driver.execute(&m.up_sql).await?;
        TableManager::update_status(&self.driver, table, &m.version, MigrationStatus::Applied).await
    }

    /// Run any repeatable migrations that are new or whose checksum changed.
    async fn apply_repeatables(
        &self,
        source: &impl MigrationSource,
        table: &str,
        mut order: i64,
        entries: &mut Vec<MigratedEntry>,
    ) -> Result<(), MigrateError> {
        let repeatables = source.load_repeatable()?;
        let applied_after = self.driver.query_applied_migrations(table).await?;

        for rm in &repeatables {
            let existing = applied_after.iter().find(|am| am.version == rm.version);
            let should_run = match existing {
                None => true,
                Some(am) => am.checksum != rm.checksum,
            };
            if !should_run {
                continue;
            }

            let start = Instant::now();
            self.driver.execute(&rm.up_sql).await?;

            if existing.is_some() {
                TableManager::update_checksum(&self.driver, table, &rm.version, &rm.checksum)
                    .await?;
            } else {
                TableManager::record_migration(&self.driver, table, rm, order).await?;
                order += 1;
            }

            entries.push(MigratedEntry {
                version: rm.version.clone(),
                name: rm.name.clone(),
                elapsed: start.elapsed(),
            });
        }

        Ok(())
    }

    /// Show migration status: applied, pending, and changed repeatables.
    ///
    /// Unlike [`migrate`](Self::migrate), `status` never errors on a
    /// PARTIAL migration or a checksum mismatch — it must keep working
    /// so the user can diagnose the problem with `db repair`.
    pub async fn status(
        &self,
        source: &impl MigrationSource,
    ) -> Result<StatusReport, MigrateError> {
        let table = &self.config.migrations_table;
        TableManager::ensure_table(&self.driver, table).await?;

        let applied = self.driver.query_applied_migrations(table).await?;
        let all_pending = source.load_versioned()?;
        let repeatables = source.load_repeatable()?;

        let applied_versions: HashSet<&str> =
            applied.iter().map(|am| am.version.as_str()).collect();

        let pending: Vec<Migration> = all_pending
            .into_iter()
            .filter(|m| !applied_versions.contains(m.version.as_str()))
            .collect();

        let repeatable_changed: Vec<Migration> = repeatables
            .into_iter()
            .filter(
                |rm| match applied.iter().find(|am| am.version == rm.version) {
                    None => true,
                    Some(am) => am.checksum != rm.checksum,
                },
            )
            .collect();

        Ok(StatusReport {
            applied,
            pending,
            repeatable_changed,
        })
    }

    /// Rollback migrations according to the given strategy.
    ///
    /// Reads stored `@down` SQL from the tracking table — rollback works
    /// even after the migration file has been deleted from disk. Refuses
    /// to run if any migration is stuck in PARTIAL state (run
    /// [`repair`](Self::repair) first), and refuses to rollback a
    /// migration whose `@down` SQL was never stored (`@down(skip)`).
    pub async fn rollback(
        &self,
        strategy: RollbackStrategy,
    ) -> Result<RollbackReport, MigrateError> {
        let start = Instant::now();
        let lock_key = self.lock_key();
        let table = self.config.migrations_table.clone();

        LockManager::acquire(&self.driver, lock_key).await?;

        let result = self.rollback_inner(strategy, &table).await;

        LockManager::release(&self.driver, lock_key).await?;

        let mut report = result?;
        report.elapsed = start.elapsed();
        Ok(report)
    }

    async fn rollback_inner(
        &self,
        strategy: RollbackStrategy,
        table: &str,
    ) -> Result<RollbackReport, MigrateError> {
        TableManager::ensure_table(&self.driver, table).await?;
        let applied = self.driver.query_applied_migrations(table).await?;

        for am in &applied {
            if am.status == MigrationStatus::Partial {
                return Err(MigrateError::PartialBlocking {
                    version: am.version.clone(),
                });
            }
        }

        let to_rollback: Vec<&AppliedMigration> = match &strategy {
            RollbackStrategy::Last => applied.last().into_iter().collect(),
            RollbackStrategy::Steps(n) => applied.iter().rev().take(*n).collect(),
            RollbackStrategy::ToVersion(v) => applied
                .iter()
                .rev()
                .take_while(|am| am.version.as_str() > v.as_str())
                .collect(),
        };

        let mut rolled_back = Vec::new();

        for am in &to_rollback {
            let Some(down_sql) = am.down_sql.as_deref() else {
                return Err(MigrateError::RollbackSkipped {
                    version: am.version.clone(),
                    reason: "no @down block stored".into(),
                });
            };

            let mut tx = self.driver.begin().await?;
            tx.execute(down_sql).await?;
            let delete_sql = format!("DELETE FROM {table} WHERE version = '{}'", am.version);
            tx.execute(&delete_sql).await?;
            tx.commit().await?;

            rolled_back.push(am.version.clone());
        }

        Ok(RollbackReport {
            rolled_back,
            elapsed: Duration::ZERO,
        })
    }

    /// Repair a migration stuck in PARTIAL state.
    ///
    /// Four strategies: retry (re-run the `@up` SQL), rollback (use the
    /// stored `@down` SQL), skip (mark PARTIAL as applied without
    /// re-running anything — a last resort), or update-checksum (fix the
    /// stored checksum after an intentional file edit; does not require a
    /// PARTIAL migration to exist).
    pub async fn repair(
        &self,
        source: &impl MigrationSource,
        strategy: RepairStrategy,
    ) -> Result<RepairReport, MigrateError> {
        let lock_key = self.lock_key();
        let table = self.config.migrations_table.clone();

        LockManager::acquire(&self.driver, lock_key).await?;

        let result = self.repair_inner(source, strategy, &table).await;

        LockManager::release(&self.driver, lock_key).await?;

        result
    }

    async fn repair_inner(
        &self,
        source: &impl MigrationSource,
        strategy: RepairStrategy,
        table: &str,
    ) -> Result<RepairReport, MigrateError> {
        TableManager::ensure_table(&self.driver, table).await?;

        if let RepairStrategy::UpdateChecksum(version) = &strategy {
            let pending = source.load_versioned()?;
            let migration = pending
                .iter()
                .find(|m| &m.version == version)
                .ok_or_else(|| MigrateError::MigrationNotFound {
                    version: version.clone(),
                })?;
            TableManager::update_checksum(&self.driver, table, version, &migration.checksum)
                .await?;
            return Ok(RepairReport {
                action: "update-checksum".into(),
                version: version.clone(),
                success: true,
            });
        }

        let applied = self.driver.query_applied_migrations(table).await?;
        let partial = applied
            .iter()
            .find(|am| am.status == MigrationStatus::Partial)
            .ok_or_else(|| MigrateError::Config("no migration in PARTIAL state".into()))?;

        let version = partial.version.clone();

        match strategy {
            RepairStrategy::Retry => {
                self.driver.execute(&partial.up_sql).await?;
                TableManager::update_status(
                    &self.driver,
                    table,
                    &version,
                    MigrationStatus::Applied,
                )
                .await?;
                Ok(RepairReport {
                    action: "retry".into(),
                    version,
                    success: true,
                })
            }
            RepairStrategy::Rollback => {
                let down_sql =
                    partial
                        .down_sql
                        .as_deref()
                        .ok_or_else(|| MigrateError::RollbackSkipped {
                            version: version.clone(),
                            reason: "no @down SQL stored".into(),
                        })?;
                self.driver.execute(down_sql).await?;
                TableManager::delete_migration(&self.driver, table, &version).await?;
                Ok(RepairReport {
                    action: "rollback".into(),
                    version,
                    success: true,
                })
            }
            RepairStrategy::Skip => {
                TableManager::update_status(
                    &self.driver,
                    table,
                    &version,
                    MigrationStatus::Applied,
                )
                .await?;
                Ok(RepairReport {
                    action: "skip".into(),
                    version,
                    success: true,
                })
            }
            RepairStrategy::UpdateChecksum(_) => unreachable!(
                "UpdateChecksum is handled above before a PARTIAL migration is required"
            ),
        }
    }

    /// Mark an existing database as being at a specific migration version.
    ///
    /// Creates the tracking table and records every migration up to and
    /// including `version` as applied, without executing any SQL. Used to
    /// adopt this tool on a database whose schema was created some other
    /// way. Fails if the tracking table already has entries — baseline is
    /// only for brand-new adoption, not for editing history.
    pub async fn baseline(
        &self,
        source: &impl MigrationSource,
        version: &str,
    ) -> Result<(), MigrateError> {
        let lock_key = self.lock_key();
        let table = self.config.migrations_table.clone();

        LockManager::acquire(&self.driver, lock_key).await?;

        let result = self.baseline_inner(source, version, &table).await;

        LockManager::release(&self.driver, lock_key).await?;

        result
    }

    async fn baseline_inner(
        &self,
        source: &impl MigrationSource,
        version: &str,
        table: &str,
    ) -> Result<(), MigrateError> {
        TableManager::ensure_table(&self.driver, table).await?;

        let existing = self.driver.query_applied_migrations(table).await?;
        if !existing.is_empty() {
            return Err(MigrateError::Config(
                "cannot baseline: migrations table already has entries".into(),
            ));
        }

        let all = source.load_versioned()?;
        let mut order = 1i64;
        for m in &all {
            if m.version.as_str() > version {
                break;
            }
            TableManager::record_migration(&self.driver, table, m, order).await?;
            order += 1;
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests {
    use super::*;
    use crate::driver::sqlite::SqliteDriver;
    use crate::source::InMemorySource;

    async fn setup() -> (MigrationEngine<SqliteDriver>, InMemorySource) {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let engine = MigrationEngine::new(driver, EngineConfig::default());
        let source = InMemorySource {
            versioned: vec![],
            repeatable: vec![],
        };
        (engine, source)
    }

    fn make_migration(version: &str, name: &str, sql: &str) -> Migration {
        Migration {
            version: version.into(),
            name: name.into(),
            up_sql: sql.into(),
            down_sql: Some(format!("DROP TABLE IF EXISTS {name};")),
            down_skip_reason: None,
            checksum: crate::checksum::compute_checksum(sql),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        }
    }

    fn make_repeatable(name: &str, sql: &str) -> Migration {
        Migration {
            version: format!("R__{name}"),
            name: name.into(),
            up_sql: sql.into(),
            down_sql: None,
            down_skip_reason: None,
            checksum: crate::checksum::compute_checksum(sql),
            no_transaction: false,
            requires: vec![],
            repeatable: true,
        }
    }

    #[tokio::test]
    async fn migrate_empty_source() {
        let (engine, source) = setup().await;
        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert!(report.applied.is_empty());
    }

    #[tokio::test]
    async fn migrate_single_migration() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_120000",
            "users",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
        ));

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].version, "20260810_120000");
        assert_eq!(report.applied[0].name, "users");
    }

    #[tokio::test]
    async fn migrate_multiple_in_order() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "users",
            "CREATE TABLE users (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "orders",
            "CREATE TABLE orders (id INTEGER);",
        ));

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 2);
        assert_eq!(report.applied[0].version, "20260810_100000");
        assert_eq!(report.applied[1].version, "20260810_120000");
    }

    #[tokio::test]
    async fn migrate_idempotent() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_120000",
            "users",
            "CREATE TABLE users (id INTEGER);",
        ));

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert!(report.applied.is_empty());
    }

    #[tokio::test]
    async fn migrate_with_target() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_140000",
            "c",
            "CREATE TABLE c (id INTEGER);",
        ));

        let report = engine
            .migrate(&source, Some("20260810_120000"), MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 2);
    }

    #[tokio::test]
    async fn migrate_dry_run() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_120000",
            "users",
            "CREATE TABLE users (id INTEGER);",
        ));

        let report = engine
            .migrate(
                &source,
                None,
                MigrateOptions {
                    dry_run: true,
                    atomic: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);

        let status = engine.status(&source).await.unwrap();
        assert!(status.applied.is_empty());
        assert_eq!(status.pending.len(), 1);
    }

    #[tokio::test]
    async fn migrate_atomic() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));

        let report = engine
            .migrate(
                &source,
                None,
                MigrateOptions {
                    dry_run: false,
                    atomic: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 2);
    }

    #[tokio::test]
    async fn migrate_atomic_rolls_back_on_failure() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "NOT VALID SQL AT ALL;",
        ));

        let err = engine
            .migrate(
                &source,
                None,
                MigrateOptions {
                    dry_run: false,
                    atomic: true,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));

        // Neither migration should have been recorded — the whole
        // transaction rolled back, including the first migration.
        let status = engine.status(&source).await.unwrap();
        assert!(status.applied.is_empty());
    }

    #[tokio::test]
    async fn migrate_sequential_failure_stops_and_keeps_prior_work() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "NOT VALID SQL AT ALL;",
        ));

        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));

        // The first migration committed in its own transaction before
        // the second one failed.
        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.applied[0].version, "20260810_100000");
    }

    #[tokio::test]
    async fn migrate_no_transaction_marks_partial_on_failure() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration("20260810_120000", "bad", "NOT VALID SQL AT ALL;");
        m.no_transaction = true;
        source.versioned.push(m);

        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.applied[0].status, MigrationStatus::Partial);
    }

    #[tokio::test]
    async fn migrate_no_transaction_succeeds() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration(
            "20260810_120000",
            "users",
            "CREATE TABLE users (id INTEGER);",
        );
        m.no_transaction = true;
        source.versioned.push(m);

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied[0].status, MigrationStatus::Applied);
    }

    #[tokio::test]
    async fn migrate_runs_new_repeatable() {
        let (engine, mut source) = setup().await;
        source
            .repeatable
            .push(make_repeatable("recalc_view", "CREATE VIEW v AS SELECT 1;"));

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].name, "recalc_view");
    }

    #[tokio::test]
    async fn migrate_reruns_repeatable_when_checksum_changes() {
        let (engine, mut source) = setup().await;
        source.repeatable.push(make_repeatable(
            "recalc_view",
            "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT 1;",
        ));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        source.repeatable[0] = make_repeatable(
            "recalc_view",
            "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT 2;",
        );
        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.applied[0].checksum, source.repeatable[0].checksum);
    }

    #[tokio::test]
    async fn migrate_skips_unchanged_repeatable() {
        let (engine, mut source) = setup().await;
        source
            .repeatable
            .push(make_repeatable("recalc_view", "CREATE VIEW v AS SELECT 1;"));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert!(report.applied.is_empty());
    }

    #[tokio::test]
    async fn migrate_repeatable_failure_leaves_checksum_untouched_and_skips_later_ones() {
        let (engine, mut source) = setup().await;
        source.repeatable.push(make_repeatable(
            "recalc_view",
            "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT 1;",
        ));
        source.repeatable.push(make_repeatable(
            "other_view",
            "DROP VIEW IF EXISTS v2; CREATE VIEW v2 AS SELECT 1;",
        ));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        let original_checksum = source.repeatable[0].checksum.clone();
        let untouched_checksum = source.repeatable[1].checksum.clone();

        // Break the first repeatable's SQL (new checksum, invalid SQL) and
        // also change the second one so we can prove it was never reached.
        source.repeatable[0] = make_repeatable("recalc_view", "NOT VALID SQL AT ALL;");
        source.repeatable[1] = make_repeatable(
            "other_view",
            "DROP VIEW IF EXISTS v2; CREATE VIEW v2 AS SELECT 2;",
        );

        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Sql(_)));

        let status = engine.status(&source).await.unwrap();

        // The failing repeatable's stored checksum is untouched — it never
        // ran successfully, so `update_checksum` must never have been
        // called for it.
        let recalc = status
            .applied
            .iter()
            .find(|a| a.name == "recalc_view")
            .unwrap();
        assert_eq!(recalc.checksum, original_checksum);

        // The repeatable after the failing one in iteration order must
        // never have been attempted either — its stored checksum is still
        // the one from the very first successful migrate() call.
        let other = status
            .applied
            .iter()
            .find(|a| a.name == "other_view")
            .unwrap();
        assert_eq!(other.checksum, untouched_checksum);

        // Both are still reported as changed, since neither's on-disk
        // checksum matches what's stored.
        assert_eq!(status.repeatable_changed.len(), 2);
    }

    #[tokio::test]
    async fn status_shows_applied_and_pending() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));

        engine
            .migrate(&source, Some("20260810_100000"), MigrateOptions::default())
            .await
            .unwrap();

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.pending.len(), 1);
        assert_eq!(status.pending[0].version, "20260810_120000");
    }

    #[tokio::test]
    async fn status_empty_db() {
        let (engine, source) = setup().await;
        let status = engine.status(&source).await.unwrap();
        assert!(status.applied.is_empty());
        assert!(status.pending.is_empty());
        assert!(status.repeatable_changed.is_empty());
    }

    #[tokio::test]
    async fn status_reports_changed_repeatable() {
        let (engine, mut source) = setup().await;
        source.repeatable.push(make_repeatable(
            "recalc_view",
            "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT 1;",
        ));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        source.repeatable[0] = make_repeatable(
            "recalc_view",
            "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT 2;",
        );
        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.repeatable_changed.len(), 1);
    }

    #[tokio::test]
    async fn migrate_empty_up_sql() {
        let (engine, mut source) = setup().await;
        source.versioned.push(Migration {
            version: "20260810_120000".into(),
            name: "noop".into(),
            up_sql: String::new(),
            down_sql: None,
            down_skip_reason: None,
            checksum: crate::checksum::compute_checksum(""),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        });

        let report = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();
        assert_eq!(report.applied.len(), 1);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
    }

    #[tokio::test]
    async fn migrate_rejects_dependency_not_met() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration("20260810_120000", "b", "CREATE TABLE b (id INTEGER);");
        m.requires = vec!["20260810_100000".into()];
        source.versioned.push(m);

        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::DependencyNotMet { .. }));
    }

    #[tokio::test]
    async fn migrate_rejects_no_transaction_in_atomic() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration("20260810_120000", "b", "CREATE TABLE b (id INTEGER);");
        m.no_transaction = true;
        source.versioned.push(m);

        let err = engine
            .migrate(
                &source,
                None,
                MigrateOptions {
                    dry_run: false,
                    atomic: true,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::NoTransactionInAtomic { .. }));
    }

    #[tokio::test]
    async fn migrate_blocked_by_partial_state() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration("20260810_100000", "bad", "NOT VALID SQL AT ALL;");
        m.no_transaction = true;
        source.versioned.push(m);
        let _ = engine
            .migrate(&source, None, MigrateOptions::default())
            .await;

        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));
        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::PartialBlocking { .. }));
    }

    #[tokio::test]
    async fn migrate_detects_checksum_mismatch() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        source.versioned[0] = make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER, extra TEXT);",
        );
        let err = engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::ChecksumMismatch { .. }));
    }

    #[tokio::test]
    async fn config_accessor_returns_configured_values() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let config = EngineConfig {
            migrations_table: "schema_history".into(),
            migrations_dir: PathBuf::from("db/migrations"),
            lock_key: Some(42),
        };
        let engine = MigrationEngine::new(driver, config);
        assert_eq!(engine.config().migrations_table, "schema_history");
        assert_eq!(engine.config().lock_key, Some(42));
    }

    #[tokio::test]
    async fn lock_key_defaults_to_derived_hash_when_unset() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let engine = MigrationEngine::new(driver, EngineConfig::default());
        let expected = LockManager::compute_lock_key("SQLite", brand::DEFAULT_TABLE);
        assert_eq!(engine.lock_key(), expected);
    }

    #[tokio::test]
    async fn lock_key_uses_configured_value_when_set() {
        let driver = SqliteDriver::connect("sqlite::memory:").await.unwrap();
        let engine = MigrationEngine::new(
            driver,
            EngineConfig {
                lock_key: Some(999),
                ..EngineConfig::default()
            },
        );
        assert_eq!(engine.lock_key(), 999);
    }

    #[tokio::test]
    async fn rollback_last() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let report = engine.rollback(RollbackStrategy::Last).await.unwrap();
        assert_eq!(report.rolled_back, vec!["20260810_120000"]);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.pending.len(), 1);
    }

    #[tokio::test]
    async fn rollback_steps() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_140000",
            "c",
            "CREATE TABLE c (id INTEGER);",
        ));

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let report = engine.rollback(RollbackStrategy::Steps(2)).await.unwrap();
        assert_eq!(report.rolled_back.len(), 2);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
    }

    #[tokio::test]
    async fn rollback_to_version() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_140000",
            "c",
            "CREATE TABLE c (id INTEGER);",
        ));

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let report = engine
            .rollback(RollbackStrategy::ToVersion("20260810_100000".into()))
            .await
            .unwrap();
        assert_eq!(report.rolled_back.len(), 2);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
        assert_eq!(status.applied[0].version, "20260810_100000");
    }

    #[tokio::test]
    async fn rollback_no_down_sql_errors() {
        let (engine, mut source) = setup().await;
        source.versioned.push(Migration {
            version: "20260810_120000".into(),
            name: "no_down".into(),
            up_sql: "CREATE TABLE x (id INTEGER);".into(),
            down_sql: None,
            down_skip_reason: Some("irreversible".into()),
            checksum: crate::checksum::compute_checksum("CREATE TABLE x (id INTEGER);"),
            no_transaction: false,
            requires: vec![],
            repeatable: false,
        });

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let err = engine.rollback(RollbackStrategy::Last).await.unwrap_err();
        assert!(matches!(err, MigrateError::RollbackSkipped { .. }));
        assert!(err.to_string().contains("no @down"));
    }

    #[tokio::test]
    async fn rollback_blocked_by_partial_state() {
        let (engine, mut source) = setup().await;
        let mut m = make_migration("20260810_100000", "bad", "NOT VALID SQL AT ALL;");
        m.no_transaction = true;
        source.versioned.push(m);
        let _ = engine
            .migrate(&source, None, MigrateOptions::default())
            .await;

        let err = engine.rollback(RollbackStrategy::Last).await.unwrap_err();
        assert!(matches!(err, MigrateError::PartialBlocking { .. }));
    }

    #[tokio::test]
    async fn rollback_empty_history_is_a_noop() {
        let (engine, _source) = setup().await;
        let report = engine.rollback(RollbackStrategy::Last).await.unwrap();
        assert!(report.rolled_back.is_empty());
    }

    #[tokio::test]
    async fn repair_retry() {
        let (engine, source) = setup().await;

        TableManager::ensure_table(&engine.driver, "_migrations")
            .await
            .unwrap();
        let m = Migration {
            version: "20260810_120000".into(),
            name: "broken".into(),
            up_sql: "CREATE TABLE IF NOT EXISTS broken (id INTEGER);".into(),
            down_sql: None,
            down_skip_reason: None,
            checksum: "abc".into(),
            no_transaction: true,
            requires: vec![],
            repeatable: false,
        };
        TableManager::record_partial(&engine.driver, "_migrations", &m, 1)
            .await
            .unwrap();

        let report = engine.repair(&source, RepairStrategy::Retry).await.unwrap();
        assert_eq!(report.action, "retry");
        assert!(report.success);
        assert_eq!(report.version, "20260810_120000");

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied[0].status, MigrationStatus::Applied);
    }

    #[tokio::test]
    async fn repair_rollback() {
        let (engine, source) = setup().await;

        TableManager::ensure_table(&engine.driver, "_migrations")
            .await
            .unwrap();
        let m = Migration {
            version: "20260810_120000".into(),
            name: "broken".into(),
            up_sql: "CREATE TABLE broken (id INTEGER);".into(),
            down_sql: Some("DROP TABLE IF EXISTS broken;".into()),
            down_skip_reason: None,
            checksum: "abc".into(),
            no_transaction: true,
            requires: vec![],
            repeatable: false,
        };
        TableManager::record_partial(&engine.driver, "_migrations", &m, 1)
            .await
            .unwrap();

        let report = engine
            .repair(&source, RepairStrategy::Rollback)
            .await
            .unwrap();
        assert_eq!(report.action, "rollback");
        assert!(report.success);

        let status = engine.status(&source).await.unwrap();
        assert!(status.applied.is_empty());
    }

    #[tokio::test]
    async fn repair_rollback_without_down_sql_errors() {
        let (engine, source) = setup().await;

        TableManager::ensure_table(&engine.driver, "_migrations")
            .await
            .unwrap();
        let m = Migration {
            version: "20260810_120000".into(),
            name: "broken".into(),
            up_sql: "CREATE TABLE broken (id INTEGER);".into(),
            down_sql: None,
            down_skip_reason: Some("irreversible".into()),
            checksum: "abc".into(),
            no_transaction: true,
            requires: vec![],
            repeatable: false,
        };
        TableManager::record_partial(&engine.driver, "_migrations", &m, 1)
            .await
            .unwrap();

        let err = engine
            .repair(&source, RepairStrategy::Rollback)
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::RollbackSkipped { .. }));
    }

    #[tokio::test]
    async fn repair_skip() {
        let (engine, source) = setup().await;

        TableManager::ensure_table(&engine.driver, "_migrations")
            .await
            .unwrap();
        let m = Migration {
            version: "20260810_120000".into(),
            name: "broken".into(),
            up_sql: "CREATE TABLE broken (id INTEGER);".into(),
            down_sql: None,
            down_skip_reason: None,
            checksum: "abc".into(),
            no_transaction: true,
            requires: vec![],
            repeatable: false,
        };
        TableManager::record_partial(&engine.driver, "_migrations", &m, 1)
            .await
            .unwrap();

        let report = engine.repair(&source, RepairStrategy::Skip).await.unwrap();
        assert_eq!(report.action, "skip");
        assert!(report.success);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied[0].status, MigrationStatus::Applied);
    }

    #[tokio::test]
    async fn repair_update_checksum() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_120000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        source.versioned[0] = make_migration(
            "20260810_120000",
            "a",
            "CREATE TABLE a (id INTEGER, extra TEXT);",
        );
        let new_checksum = source.versioned[0].checksum.clone();

        let report = engine
            .repair(
                &source,
                RepairStrategy::UpdateChecksum("20260810_120000".into()),
            )
            .await
            .unwrap();
        assert_eq!(report.action, "update-checksum");
        assert!(report.success);

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied[0].checksum, new_checksum);
    }

    #[tokio::test]
    async fn repair_update_checksum_missing_migration_errors() {
        let (engine, source) = setup().await;

        let err = engine
            .repair(
                &source,
                RepairStrategy::UpdateChecksum("20260810_120000".into()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::MigrationNotFound { .. }));
    }

    #[tokio::test]
    async fn repair_without_partial_migration_errors() {
        let (engine, source) = setup().await;

        let err = engine
            .repair(&source, RepairStrategy::Skip)
            .await
            .unwrap_err();
        assert!(matches!(err, MigrateError::Config(_)));
        assert!(err.to_string().contains("PARTIAL"));
    }

    #[tokio::test]
    async fn baseline_records_up_to_version() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_120000",
            "b",
            "CREATE TABLE b (id INTEGER);",
        ));
        source.versioned.push(make_migration(
            "20260810_140000",
            "c",
            "CREATE TABLE c (id INTEGER);",
        ));

        engine.baseline(&source, "20260810_120000").await.unwrap();

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 2);
        assert_eq!(status.pending.len(), 1);
        assert_eq!(status.pending[0].version, "20260810_140000");
    }

    #[tokio::test]
    async fn baseline_fails_if_table_has_entries() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "CREATE TABLE a (id INTEGER);",
        ));

        engine
            .migrate(&source, None, MigrateOptions::default())
            .await
            .unwrap();

        let err = engine
            .baseline(&source, "20260810_100000")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already has entries"));
    }

    #[tokio::test]
    async fn baseline_does_not_execute_sql() {
        let (engine, mut source) = setup().await;
        source.versioned.push(make_migration(
            "20260810_100000",
            "a",
            "NOT VALID SQL AT ALL;",
        ));

        // baseline records the migration as applied without running its
        // SQL, so even invalid SQL succeeds.
        engine.baseline(&source, "20260810_100000").await.unwrap();

        let status = engine.status(&source).await.unwrap();
        assert_eq!(status.applied.len(), 1);
    }
}
