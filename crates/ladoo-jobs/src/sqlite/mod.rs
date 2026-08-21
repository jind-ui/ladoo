//! SQLite backend for the job queue.

pub(crate) mod migrations;

/// SQLite-backed [`JobStore`](crate::JobStore) implementation.
pub struct SqliteStore;
