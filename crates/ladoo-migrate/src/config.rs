//! Multi-database configuration via `migrate.toml`.
//!
//! When a project has multiple databases, `migrate.toml` defines each
//! one with its own environment variable for the URL, migrations
//! directory, and optional custom table name.
//!
//! # Example
//!
//! ```toml
//! [databases.primary]
//! url_env = "PRIMARY_DATABASE_URL"
//! migrations = "migrations/primary"
//!
//! [databases.analytics]
//! url_env = "ANALYTICS_DATABASE_URL"
//! migrations = "migrations/analytics"
//! table = "schema_history"
//! ```

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::brand::{DEFAULT_DIR, DEFAULT_TABLE};
use crate::MigrateError;

/// Top-level `migrate.toml` configuration.
///
/// Contains a map of named database configurations. Each database
/// operates independently with its own migrations directory, tracking
/// table, and connection URL.
#[derive(Debug, Deserialize)]
pub struct MigrateConfig {
    /// Named database configurations.
    pub databases: HashMap<String, DatabaseConfig>,
}

/// Configuration for a single database in `migrate.toml`.
#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    /// Name of the environment variable containing the database URL.
    pub url_env: String,
    /// Path to the migrations directory for this database.
    ///
    /// Defaults to [`DEFAULT_DIR`] when omitted from `migrate.toml`.
    #[serde(default = "default_migrations_dir")]
    pub migrations: String,
    /// Optional custom table name (defaults to [`DEFAULT_TABLE`]).
    pub table: Option<String>,
}

fn default_migrations_dir() -> String {
    DEFAULT_DIR.to_string()
}

impl MigrateConfig {
    /// Load configuration from a `migrate.toml` file.
    ///
    /// Returns [`MigrateError::Config`] if the file is missing or invalid.
    pub fn load(path: &Path) -> Result<Self, MigrateError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| MigrateError::Config(format!("cannot read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| MigrateError::Config(format!("invalid migrate.toml: {e}")))
    }

    /// Get a specific database configuration by name.
    pub fn get(&self, name: &str) -> Option<&DatabaseConfig> {
        self.databases.get(name)
    }

    /// Returns all database names.
    pub fn names(&self) -> Vec<&str> {
        self.databases.keys().map(|k| k.as_str()).collect()
    }
}

impl DatabaseConfig {
    /// Resolve the database URL by reading the configured environment variable.
    ///
    /// Returns [`MigrateError::Config`] if the env var is not set.
    pub fn resolve_url(&self) -> Result<String, MigrateError> {
        std::env::var(&self.url_env).map_err(|_| {
            MigrateError::Config(format!(
                "environment variable {} is not set",
                self.url_env
            ))
        })
    }

    /// Returns the effective tracking table name.
    ///
    /// Falls back to [`DEFAULT_TABLE`] when `table` is not set.
    pub fn table_name(&self) -> &str {
        self.table.as_deref().unwrap_or(DEFAULT_TABLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_valid_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("migrate.toml");
        std::fs::write(
            &path,
            r#"
[databases.primary]
url_env = "PRIMARY_DATABASE_URL"
migrations = "migrations/primary"

[databases.analytics]
url_env = "ANALYTICS_DATABASE_URL"
migrations = "migrations/analytics"
table = "schema_history"
"#,
        )
        .unwrap();

        let config = MigrateConfig::load(&path).unwrap();
        assert_eq!(config.databases.len(), 2);

        let primary = config.get("primary").unwrap();
        assert_eq!(primary.url_env, "PRIMARY_DATABASE_URL");
        assert_eq!(primary.migrations, "migrations/primary");
        assert!(primary.table.is_none());

        let analytics = config.get("analytics").unwrap();
        assert_eq!(analytics.table.as_deref(), Some("schema_history"));
    }

    #[test]
    fn missing_file_returns_error() {
        let err = MigrateConfig::load(Path::new("/nonexistent/migrate.toml")).unwrap_err();
        assert!(err.to_string().contains("cannot read"));
    }

    #[test]
    fn invalid_toml_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("migrate.toml");
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let err = MigrateConfig::load(&path).unwrap_err();
        assert!(err.to_string().contains("invalid migrate.toml"));
    }

    #[test]
    fn resolve_url_from_env() {
        let db = DatabaseConfig {
            url_env: "TEST_LADOO_MIGRATE_URL_12345".into(),
            migrations: "migrations".into(),
            table: None,
        };

        // Set env var for test
        std::env::set_var("TEST_LADOO_MIGRATE_URL_12345", "sqlite::memory:");
        let url = db.resolve_url().unwrap();
        assert_eq!(url, "sqlite::memory:");
        std::env::remove_var("TEST_LADOO_MIGRATE_URL_12345");
    }

    #[test]
    fn resolve_url_missing_env_returns_error() {
        let db = DatabaseConfig {
            url_env: "NONEXISTENT_VAR_LADOO_TEST".into(),
            migrations: "migrations".into(),
            table: None,
        };
        let err = db.resolve_url().unwrap_err();
        assert!(err.to_string().contains("not set"));
    }

    #[test]
    fn names_returns_all_databases() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("migrate.toml");
        std::fs::write(
            &path,
            r#"
[databases.a]
url_env = "A"
migrations = "a"

[databases.b]
url_env = "B"
migrations = "b"
"#,
        )
        .unwrap();

        let config = MigrateConfig::load(&path).unwrap();
        let mut names = config.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("migrate.toml");
        std::fs::write(
            &path,
            r#"
[databases.primary]
url_env = "X"
migrations = "m"
"#,
        )
        .unwrap();

        let config = MigrateConfig::load(&path).unwrap();
        assert!(config.get("nonexistent").is_none());
    }

    #[test]
    fn table_name_defaults_when_unset() {
        let db = DatabaseConfig {
            url_env: "X".into(),
            migrations: "m".into(),
            table: None,
        };
        assert_eq!(db.table_name(), DEFAULT_TABLE);
    }

    #[test]
    fn table_name_uses_custom_value() {
        let db = DatabaseConfig {
            url_env: "X".into(),
            migrations: "m".into(),
            table: Some("custom_table".into()),
        };
        assert_eq!(db.table_name(), "custom_table");
    }

    #[test]
    fn migrations_defaults_when_omitted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("migrate.toml");
        std::fs::write(
            &path,
            r#"
[databases.primary]
url_env = "X"
"#,
        )
        .unwrap();

        let config = MigrateConfig::load(&path).unwrap();
        let primary = config.get("primary").unwrap();
        assert_eq!(primary.migrations, DEFAULT_DIR);
    }
}
