//! CLI entry point and command definitions.
//!
//! The CLI is feature-gated behind `cli`. It provides the `db` subcommand
//! group with commands: `migrate`, `rollback`, `status`, `create`,
//! `repair`, and `baseline`.

pub mod baseline;
pub mod create;
pub mod migrate;
pub mod repair;
pub mod rollback;
pub mod status;

use clap::{Parser, Subcommand};

use crate::brand;

/// Build the `--help`/`-h` summary text, combining [`brand::DISPLAY_NAME`]
/// with a one-line description of what the tool does.
fn about_text() -> String {
    format!("{} — a standalone SQL migration engine", brand::DISPLAY_NAME)
}

/// Top-level CLI application.
#[derive(Parser)]
#[command(
    name = brand::CRATE_NAME,
    version = brand::VERSION,
    about = about_text()
)]
pub struct Cli {
    /// Database connection URL (overrides DATABASE_URL env).
    #[arg(long, global = true)]
    pub database_url: Option<String>,

    /// Read database URL from a file (for Docker secrets).
    #[arg(long, global = true)]
    pub database_url_file: Option<String>,

    /// Migrations directory (default: migrations/).
    #[arg(long, global = true, default_value = brand::DEFAULT_DIR)]
    pub migrations_dir: String,

    /// Migrations table name (default: _migrations).
    #[arg(long, global = true, default_value = brand::DEFAULT_TABLE)]
    pub table: String,

    /// Target a specific database from migrate.toml.
    #[arg(long, global = true)]
    pub name: Option<String>,

    /// Run against all databases in migrate.toml.
    #[arg(long, global = true)]
    pub all: bool,

    /// The db subcommand group.
    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Commands {
    /// Apply pending migrations.
    Migrate(migrate::MigrateArgs),
    /// Rollback applied migrations.
    Rollback(rollback::RollbackArgs),
    /// Show migration status.
    Status(status::StatusArgs),
    /// Create a new migration file.
    Create(create::CreateArgs),
    /// Repair a PARTIAL-state migration.
    Repair(repair::RepairArgs),
    /// Mark existing DB as baseline.
    Baseline(baseline::BaselineArgs),
}

/// Resolve the database URL from CLI args or environment.
///
/// Checked in order: `--database-url`, `--database-url-file`, the
/// `DATABASE_URL` environment variable. Returns
/// [`MigrateError::Config`](crate::MigrateError::Config) if none are set.
pub fn resolve_database_url(cli: &Cli) -> Result<String, crate::MigrateError> {
    if let Some(url) = &cli.database_url {
        if url.contains('@') || url.contains("://") {
            eprintln!("Warning: credentials visible in command line — prefer DATABASE_URL env var");
        }
        return Ok(url.clone());
    }

    if let Some(path) = &cli.database_url_file {
        let content = std::fs::read_to_string(path)?;
        return Ok(content.trim().to_string());
    }

    std::env::var("DATABASE_URL").map_err(|_| {
        crate::MigrateError::Config(
            "DATABASE_URL not set — provide --database-url, --database-url-file, or set DATABASE_URL".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `resolve_database_url` falls back to the process-global `DATABASE_URL`
    // env var, so tests that touch it must not run concurrently with each
    // other (cargo test runs tests in the same process by default). This
    // guard serializes just the tests in this module; no other test in the
    // crate reads or writes `DATABASE_URL`.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    fn cli_with(database_url: Option<&str>, database_url_file: Option<&str>) -> Cli {
        Cli {
            database_url: database_url.map(String::from),
            database_url_file: database_url_file.map(String::from),
            migrations_dir: brand::DEFAULT_DIR.to_string(),
            table: brand::DEFAULT_TABLE.to_string(),
            name: None,
            all: false,
            command: Commands::Status(status::StatusArgs {
                format: "table".into(),
            }),
        }
    }

    #[test]
    fn explicit_url_without_credentials_is_used_as_is() {
        let cli = cli_with(Some("sqlite::memory:"), None);
        assert_eq!(resolve_database_url(&cli).unwrap(), "sqlite::memory:");
    }

    #[test]
    fn explicit_url_with_credentials_still_resolves() {
        let cli = cli_with(Some("postgres://user:pass@localhost/db"), None);
        assert_eq!(
            resolve_database_url(&cli).unwrap(),
            "postgres://user:pass@localhost/db"
        );
    }

    #[test]
    fn url_file_is_read_and_trimmed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("db_url.txt");
        std::fs::write(&path, "sqlite::memory:\n").unwrap();

        let cli = cli_with(None, Some(path.to_str().unwrap()));
        assert_eq!(resolve_database_url(&cli).unwrap(), "sqlite::memory:");
    }

    #[test]
    fn url_file_missing_returns_io_error() {
        let cli = cli_with(None, Some("/nonexistent/db_url.txt"));
        let err = resolve_database_url(&cli).unwrap_err();
        assert!(matches!(err, crate::MigrateError::Io(_)));
    }

    #[test]
    fn database_url_takes_precedence_over_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("db_url.txt");
        std::fs::write(&path, "sqlite:should_not_be_used.db").unwrap();

        let cli = cli_with(Some("sqlite::memory:"), Some(path.to_str().unwrap()));
        assert_eq!(resolve_database_url(&cli).unwrap(), "sqlite::memory:");
    }

    #[test]
    fn falls_back_to_env_var_when_no_flags_set() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("DATABASE_URL", "sqlite::memory:");
        let cli = cli_with(None, None);
        let result = resolve_database_url(&cli);
        std::env::remove_var("DATABASE_URL");
        assert_eq!(result.unwrap(), "sqlite::memory:");
    }

    #[test]
    fn errors_when_nothing_is_set() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("DATABASE_URL");
        let cli = cli_with(None, None);
        let err = resolve_database_url(&cli).unwrap_err();
        assert!(err.to_string().contains("DATABASE_URL not set"));
    }

    #[test]
    fn about_text_includes_display_name() {
        assert!(about_text().starts_with(brand::DISPLAY_NAME));
    }

    #[test]
    fn cli_parses_migrate_subcommand() {
        let cli = Cli::parse_from(["ladoo-migrate", "migrate", "--dry-run"]);
        match cli.command {
            Commands::Migrate(args) => assert!(args.dry_run),
            _ => panic!("expected Migrate command"),
        }
    }

    #[test]
    fn cli_parses_global_flags() {
        let cli = Cli::parse_from([
            "ladoo-migrate",
            "--database-url",
            "sqlite::memory:",
            "--table",
            "custom_migrations",
            "status",
        ]);
        assert_eq!(cli.database_url.as_deref(), Some("sqlite::memory:"));
        assert_eq!(cli.table, "custom_migrations");
    }
}
