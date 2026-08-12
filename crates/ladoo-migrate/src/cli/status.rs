//! `db status` command implementation.

use clap::Args;

/// Arguments for `db status`.
#[derive(Args)]
pub struct StatusArgs {
    /// Output format.
    #[arg(long, default_value = "table")]
    pub format: String,
}
