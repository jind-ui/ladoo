//! `db baseline` command implementation.

use clap::Args;

/// Arguments for `db baseline`.
#[derive(Args)]
pub struct BaselineArgs {
    /// Version to baseline to.
    pub version: String,
}
