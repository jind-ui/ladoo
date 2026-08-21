//! SQL migration strings for SQLite.

/// Creates the `_ladoo_jobs` table if it does not already exist.
pub const CREATE_JOBS_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS _ladoo_jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    run_at      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    last_error  TEXT,
    locked_by   TEXT,
    locked_at   TEXT
)";

/// Creates an index to speed up polling for claimable jobs.
pub const CREATE_JOBS_INDEX: &str = "\
CREATE INDEX IF NOT EXISTS idx_ladoo_jobs_poll
    ON _ladoo_jobs (run_at)
    WHERE status IN ('pending', 'scheduled')";

/// Creates the `_ladoo_cron_schedules` table if it does not already exist.
pub const CREATE_CRON_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS _ladoo_cron_schedules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    schedule    TEXT NOT NULL,
    last_run    TEXT,
    next_run    TEXT NOT NULL
)";
