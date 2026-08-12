//! `ladoo-migrate` CLI binary.
//!
//! Parses arguments with [`clap`], resolves the database connection and
//! migration source, builds a [`MigrationEngine`], and dispatches to the
//! requested subcommand. See [`ladoo_migrate::cli`] for the command
//! definitions.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use ladoo_migrate::cli::{resolve_database_url, Cli, Commands};
use ladoo_migrate::driver::sqlite::SqliteDriver;
use ladoo_migrate::driver::MigrationDriver;
use ladoo_migrate::engine::{
    EngineConfig, MigrateOptions, MigrationEngine, RepairStrategy, RollbackStrategy,
};
use ladoo_migrate::source::filesystem::FilesystemSource;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, ladoo_migrate::MigrateError> {
    let url = resolve_database_url(&cli)?;
    let config = EngineConfig {
        migrations_table: cli.table.clone(),
        migrations_dir: PathBuf::from(&cli.migrations_dir),
        lock_key: None,
    };
    let source = FilesystemSource::new(&cli.migrations_dir);

    // For now, auto-detect driver from URL scheme is not implemented —
    // the CLI always connects with the SQLite driver. In future, add a
    // `--driver` flag and dispatch to Postgres/MySQL drivers (Task 11).
    let driver = SqliteDriver::connect(&url).await?;
    let engine = MigrationEngine::new(driver, config);

    match cli.command {
        Commands::Migrate(args) => {
            let opts = MigrateOptions {
                dry_run: args.dry_run,
                atomic: args.atomic,
            };
            let report = engine.migrate(&source, args.to.as_deref(), opts).await?;

            if report.applied.is_empty() {
                println!("Nothing to migrate.");
            } else {
                for entry in &report.applied {
                    let prefix = if args.dry_run { "[dry-run] " } else { "" };
                    println!(
                        "{prefix}Applied {} ({}) in {:?}",
                        entry.version, entry.name, entry.elapsed
                    );
                }
                println!(
                    "\n{} migration(s) applied in {:?}",
                    report.applied.len(),
                    report.elapsed
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Rollback(args) => {
            let strategy = if let Some(steps) = args.steps {
                RollbackStrategy::Steps(steps)
            } else if let Some(to) = args.to {
                RollbackStrategy::ToVersion(to)
            } else {
                RollbackStrategy::Last
            };

            let report = engine.rollback(strategy).await?;
            for v in &report.rolled_back {
                println!("Rolled back {v}");
            }
            println!(
                "\n{} migration(s) rolled back in {:?}",
                report.rolled_back.len(),
                report.elapsed
            );
            Ok(ExitCode::SUCCESS)
        }
        Commands::Status(args) => {
            let status = engine.status(&source).await?;

            if args.format == "json" {
                let applied: Vec<&str> =
                    status.applied.iter().map(|a| a.version.as_str()).collect();
                let pending: Vec<&str> =
                    status.pending.iter().map(|p| p.version.as_str()).collect();
                println!(
                    "{{\"applied\":{},\"pending\":{}}}",
                    serde_json::to_string(&applied).unwrap_or_default(),
                    serde_json::to_string(&pending).unwrap_or_default(),
                );
            } else {
                println!("Applied ({}):", status.applied.len());
                for am in &status.applied {
                    let status_marker = if am.status == ladoo_migrate::MigrationStatus::Partial {
                        " [PARTIAL]"
                    } else {
                        ""
                    };
                    println!("  {} ({}){}", am.version, am.name, status_marker);
                }
                println!("\nPending ({}):", status.pending.len());
                for m in &status.pending {
                    println!("  {} ({})", m.version, m.name);
                }
                if !status.repeatable_changed.is_empty() {
                    println!(
                        "\nRepeatable (changed) ({}):",
                        status.repeatable_changed.len()
                    );
                    for rm in &status.repeatable_changed {
                        println!("  {}", rm.name);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Create(args) => {
            if let Some(revert_version) = args.revert {
                let status = engine.status(&source).await?;
                let am = status
                    .applied
                    .iter()
                    .find(|a| a.version == revert_version)
                    .ok_or_else(|| ladoo_migrate::MigrateError::MigrationNotFound {
                        version: revert_version.clone(),
                    })?;
                let down_sql = am.down_sql.as_deref().ok_or_else(|| {
                    ladoo_migrate::MigrateError::RollbackSkipped {
                        version: revert_version.clone(),
                        reason: "no @down SQL stored".into(),
                    }
                })?;
                let filename = ladoo_migrate::cli::create::generate_filename(&format!(
                    "revert_{revert_version}"
                ));
                let content = format!(
                    "-- @up\n-- Forward-fix: reverting {revert_version}\n{down_sql}\n\n-- @down(skip) Generated revert migration"
                );
                println!("Generated migration:\n\n{content}\n");
                let path = ladoo_migrate::cli::create::write_migration(
                    std::path::Path::new(&cli.migrations_dir),
                    &filename,
                    &content,
                )?;
                println!("Written to: {}", path.display());
            } else {
                let name = args.name.ok_or_else(|| {
                    ladoo_migrate::MigrateError::Config("migration name required".into())
                })?;
                let content = if let Some(template_type) = &args.r#type {
                    ladoo_migrate::cli::create::template_content(
                        template_type,
                        args.table.as_deref(),
                    )?
                } else {
                    "-- @up\n\n-- @down\n".to_string()
                };
                let filename = ladoo_migrate::cli::create::generate_filename(&name);
                let dir = std::path::Path::new(&cli.migrations_dir);
                if !dir.exists() {
                    std::fs::create_dir_all(dir)?;
                }
                let path = ladoo_migrate::cli::create::write_migration(dir, &filename, &content)?;
                println!("Created: {}", path.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Repair(args) => {
            let strategy = if args.retry {
                RepairStrategy::Retry
            } else if args.rollback {
                RepairStrategy::Rollback
            } else if args.skip {
                RepairStrategy::Skip
            } else if let Some(version) = args.update_checksum {
                RepairStrategy::UpdateChecksum(version)
            } else {
                return Err(ladoo_migrate::MigrateError::Config(
                    "specify one of --retry, --rollback, --skip, or --update-checksum".into(),
                ));
            };

            let report = engine.repair(&source, strategy).await?;
            println!(
                "Repair {}: {} (version: {})",
                if report.success { "succeeded" } else { "failed" },
                report.action,
                report.version
            );
            Ok(ExitCode::SUCCESS)
        }
        Commands::Baseline(args) => {
            engine.baseline(&source, &args.version).await?;
            println!("Baselined at version {}", args.version);
            Ok(ExitCode::SUCCESS)
        }
    }
}
