#![warn(missing_docs)]

//! # Ladoo Migrate
//!
//! A standalone SQL migration engine that replaces Flyway with better
//! performance, smaller footprint, and a developer-friendly file format.
//!
//! This crate has **zero dependency** on the Ladoo web framework — it is
//! independently publishable on crates.io and usable by any Rust project.
//!
//! ## Quick Start (Library)
//!
//! ```rust,ignore
//! use ladoo_migrate::{MigrationEngine, EngineConfig};
//! use ladoo_migrate::driver::SqliteDriver;
//!
//! let driver = SqliteDriver::connect("sqlite::memory:").await?;
//! let engine = MigrationEngine::new(driver, EngineConfig::default());
//! let report = engine.migrate(None, Default::default()).await?;
//! println!("Applied {} migrations", report.applied.len());
//! ```
//!
//! ## Quick Start (CLI)
//!
//! ```bash
//! # Apply all pending migrations
//! ladoo-migrate migrate
//!
//! # Show migration status
//! ladoo-migrate status
//!
//! # Create a new migration
//! ladoo-migrate create add_users_table
//! ```

pub mod brand;
pub mod checksum;
#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
pub mod driver;
pub mod engine;
pub mod error;
pub mod lock;
pub mod migration;
pub mod plan;
pub mod source;
pub mod table;

pub use checksum::compute_checksum;
pub use config::{DatabaseConfig, MigrateConfig};
pub use engine::{
    EngineConfig, MigrateOptions, MigrateReport, MigratedEntry, MigrationEngine, RepairReport,
    RepairStrategy, RollbackReport, RollbackStrategy, StatusReport,
};
pub use error::MigrateError;
pub use migration::{AppliedMigration, Migration, MigrationStatus};
pub use plan::MigrationPlan;
pub use source::filesystem::FilesystemSource;
pub use source::{InMemorySource, MigrationSource};

#[cfg(feature = "sqlite")]
pub use driver::sqlite::SqliteDriver;

/// Convenience alias for results using [`MigrateError`].
pub type Result<T> = std::result::Result<T, MigrateError>;
