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
//! ladoo-migrate db migrate
//!
//! # Show migration status
//! ladoo-migrate db status
//!
//! # Create a new migration
//! ladoo-migrate db create add_users_table
//! ```

pub mod brand;
pub mod error;

pub use error::MigrateError;

/// Convenience alias for results using [`MigrateError`].
pub type Result<T> = std::result::Result<T, MigrateError>;
