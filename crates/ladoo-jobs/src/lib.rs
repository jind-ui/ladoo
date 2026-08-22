//! Database-backed persistent job queue.
//!
//! `ladoo-jobs` stores background jobs in Postgres or SQLite, with
//! configurable retries, scheduling (`delay`, `at`, `cron`), and
//! a transactional outbox for guaranteed consistency. It has no
//! dependency on the `ladoo` web framework and can be used standalone
//! in any Tokio-based Rust application.
//!
//! # Backend Selection
//!
//! Enable exactly one backend via Cargo features:
//!
//! ```toml
//! ladoo-jobs = { version = "0.1", features = ["postgres"] }
//! ladoo-jobs = { version = "0.1", features = ["sqlite"] }
//! ```
//!
//! # Quick Start
//!
//! Define a job by implementing [`PersistentJob`], enqueue it through
//! [`JobStoreExt`], and run a [`Worker`] to process it. This example is
//! generic over the backend (`S: JobStore`) — in practice `S` is a
//! `SqliteStore` or `PostgresStore` built with the `sqlite` or `postgres`
//! feature:
//!
//! ```no_run
//! use std::future::Future;
//! use std::pin::Pin;
//!
//! use ladoo_jobs::{JobRegistry, JobStore, JobStoreExt, PersistentJob, Worker};
//!
//! #[derive(serde::Serialize, serde::Deserialize)]
//! struct SendWelcomeEmail {
//!     user_id: u64,
//! }
//!
//! impl PersistentJob for SendWelcomeEmail {
//!     fn name() -> &'static str {
//!         "send_welcome_email"
//!     }
//!     fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
//!         Box::pin(async move {
//!             // send the email...
//!             Ok(())
//!         })
//!     }
//! }
//!
//! # async fn run<S: JobStore + 'static>(store: S) -> Result<(), Box<dyn std::error::Error>> {
//! store.migrate().await?;
//!
//! // Enqueue for immediate execution (see also `enqueue_delayed` and `enqueue_at`).
//! store.enqueue(&SendWelcomeEmail { user_id: 42 }).await?;
//!
//! // Register handlers and run the worker loop until shutdown.
//! let mut registry = JobRegistry::new();
//! registry.register::<SendWelcomeEmail>();
//!
//! let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
//! let worker = Worker::new(store, registry);
//! worker.run(shutdown_rx).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Recurring jobs can be scheduled with [`CronScheduler`], which inserts
//! jobs into the store on a cron expression for a [`Worker`] to pick up.
//!
//! # Design
//!
//! - **Storage**: [`JobStore`] is the core trait; `PostgresStore` and
//!   `SqliteStore` (enabled via the `postgres`/`sqlite` features) are the
//!   built-in backends. Implement it yourself to support another database.
//! - **Dispatch**: [`JobRegistry`] maps a job's [`PersistentJob::name`] to
//!   its deserializer and handler; [`Worker`] polls the store, claims jobs,
//!   and dispatches them concurrently.
//! - **Scheduling**: [`JobStoreExt`] provides `enqueue`, `enqueue_delayed`,
//!   and `enqueue_at` for one-off jobs; [`CronScheduler`] handles recurring
//!   jobs on a cron expression.
//! - **Consistency**: the [`outbox`] module supports the transactional
//!   outbox pattern, so a job can be enqueued atomically alongside other
//!   database writes in the same transaction.

pub mod error;
pub mod outbox;
pub mod registry;
pub mod scheduler;
pub mod store;
pub mod worker;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod worker_identity {
    use std::sync::atomic::{AtomicU64, Ordering};

    static CLAIM_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique worker identifier for each `claim()` call.
    ///
    /// Combines the process ID with a monotonic counter so that multiple
    /// Workers in the same process never share an ID.
    pub(crate) fn worker_id() -> String {
        let seq = CLAIM_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("worker-{}-{seq}", std::process::id())
    }
}
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use worker_identity::worker_id;

pub use error::JobStoreError;
pub use registry::{JobRegistry, PersistentJob};
pub use scheduler::{CronError, CronScheduler};
pub use store::{JobId, JobStatus, JobStore, JobStoreExt, NewJob, QueuedJob};
pub use worker::Worker;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(feature = "postgres")]
pub use outbox::enqueue_outbox_pg;

#[cfg(feature = "sqlite")]
pub use outbox::enqueue_outbox_sqlite;
