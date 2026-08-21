//! SQL migration strings for Postgres.

/// Creates the `_ladoo_jobs` table if it does not already exist.
pub const CREATE_JOBS_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS _ladoo_jobs (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL,
    payload     JSONB NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INT NOT NULL DEFAULT 0,
    max_retries INT NOT NULL DEFAULT 3,
    run_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_error  TEXT,
    locked_by   TEXT,
    locked_at   TIMESTAMPTZ
)";

/// Creates an index to speed up polling for claimable jobs.
pub const CREATE_JOBS_INDEX: &str = "\
CREATE INDEX IF NOT EXISTS idx_ladoo_jobs_poll
    ON _ladoo_jobs (run_at)
    WHERE status IN ('pending', 'scheduled')";

/// Creates the `_ladoo_cron_schedules` table if it does not already exist.
pub const CREATE_CRON_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS _ladoo_cron_schedules (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    schedule    TEXT NOT NULL,
    last_run    TIMESTAMPTZ,
    next_run    TIMESTAMPTZ NOT NULL
)";

/// Creates (or replaces) the trigger function that notifies
/// `_ladoo_jobs_channel` whenever a row is inserted into `_ladoo_jobs`.
pub const NOTIFY_FUNCTION: &str = "\
CREATE OR REPLACE FUNCTION _ladoo_jobs_notify() RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('_ladoo_jobs_channel', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql";

/// Creates the `AFTER INSERT` trigger that invokes [`NOTIFY_FUNCTION`],
/// if it does not already exist.
pub const NOTIFY_TRIGGER: &str = "\
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger WHERE tgname = '_ladoo_jobs_insert_notify'
    ) THEN
        CREATE TRIGGER _ladoo_jobs_insert_notify
            AFTER INSERT ON _ladoo_jobs
            FOR EACH ROW EXECUTE FUNCTION _ladoo_jobs_notify();
    END IF;
END $$";
