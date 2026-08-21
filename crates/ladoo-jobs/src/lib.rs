//! Database-backed persistent job queue.
//!
//! `ladoo-jobs` stores background jobs in Postgres or SQLite, with
//! configurable retries, scheduling (`delay`, `at`, `cron`), and
//! a transactional outbox for guaranteed consistency.
//!
//! # Backend Selection
//!
//! Enable exactly one backend via Cargo features:
//!
//! ```toml
//! ladoo-jobs = { version = "0.1", features = ["postgres"] }
//! ladoo-jobs = { version = "0.1", features = ["sqlite"] }
//! ```

pub mod error;
pub mod registry;
pub mod store;
pub mod worker;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use error::JobStoreError;
pub use registry::{JobRegistry, PersistentJob};
pub use store::{JobId, JobStatus, JobStore, NewJob, QueuedJob};
pub use worker::Worker;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;
