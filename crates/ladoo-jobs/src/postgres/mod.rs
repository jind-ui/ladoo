//! Postgres backend for the job queue.

pub(crate) mod migrations;

/// Postgres-backed [`JobStore`](crate::JobStore) implementation.
pub struct PostgresStore;
