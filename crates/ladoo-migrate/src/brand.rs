//! Centralized branding constants for easy renaming.
//!
//! All user-facing output (CLI help, error messages, logging) references
//! these constants. To rename the tool: update `Cargo.toml`, change
//! [`DISPLAY_NAME`], and update the `[[bin]]` section.

/// Crate name from `Cargo.toml`.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

/// Human-readable display name for CLI output and error messages.
pub const DISPLAY_NAME: &str = "Ladoo Migrate";

/// Crate version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default name of the migrations tracking table.
pub const DEFAULT_TABLE: &str = "_migrations";

/// Default directory for migration files.
pub const DEFAULT_DIR: &str = "migrations";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_matches_cargo_toml() {
        assert_eq!(CRATE_NAME, "ladoo-migrate");
    }

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn default_table_is_underscore_prefixed() {
        assert_eq!(DEFAULT_TABLE, "_migrations");
    }

    #[test]
    fn default_dir_is_migrations() {
        assert_eq!(DEFAULT_DIR, "migrations");
    }
}
