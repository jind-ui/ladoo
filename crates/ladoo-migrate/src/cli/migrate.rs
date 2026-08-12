//! `db migrate` command implementation.

use clap::Args;

/// Arguments for `db migrate`.
#[derive(Args)]
pub struct MigrateArgs {
    /// Apply up to this version (inclusive) and stop.
    #[arg(long)]
    pub to: Option<String>,

    /// Show what would be applied without executing.
    #[arg(long)]
    pub dry_run: bool,

    /// Run all pending migrations in a single transaction.
    #[arg(long)]
    pub atomic: bool,
}
