//! `db repair` command implementation.

use clap::Args;

/// Arguments for `db repair`.
#[derive(Args)]
pub struct RepairArgs {
    /// Re-run the PARTIAL migration.
    #[arg(long)]
    pub retry: bool,

    /// Rollback using stored @down SQL.
    #[arg(long)]
    pub rollback: bool,

    /// Mark PARTIAL as applied (last resort).
    #[arg(long)]
    pub skip: bool,

    /// Update stored checksum for a specific version.
    #[arg(long)]
    pub update_checksum: Option<String>,
}
