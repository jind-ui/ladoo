//! `db rollback` command implementation.

use clap::Args;

/// Arguments for `db rollback`.
#[derive(Args)]
pub struct RollbackArgs {
    /// Rollback the last N migrations.
    #[arg(long)]
    pub steps: Option<usize>,

    /// Rollback all migrations after this version.
    #[arg(long)]
    pub to: Option<String>,
}
