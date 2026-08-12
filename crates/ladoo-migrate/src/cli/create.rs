//! `db create` command implementation.

use std::path::Path;

use chrono::Utc;
use clap::Args;

use crate::MigrateError;

/// Arguments for `db create`.
#[derive(Args)]
pub struct CreateArgs {
    /// Name for the new migration.
    pub name: Option<String>,

    /// Template type for pre-filled content.
    #[arg(long, value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Table name for templates that need one.
    #[arg(long)]
    pub table: Option<String>,

    /// Generate a forward-fix from stored @down SQL.
    #[arg(long)]
    pub revert: Option<String>,
}

/// Generate a migration filename with UTC timestamp.
pub fn generate_filename(name: &str) -> String {
    let now = Utc::now();
    format!("{}_{}.sql", now.format("%Y%m%d_%H%M%S"), name)
}

/// Generate template content for a given type.
///
/// Returns [`MigrateError::Config`] for an unrecognized `template_type`.
pub fn template_content(
    template_type: &str,
    table_name: Option<&str>,
) -> Result<String, MigrateError> {
    let tbl = table_name.unwrap_or("TABLE_NAME");
    match template_type {
        "create-table" => Ok(format!(
            "-- @up\nCREATE TABLE {tbl} (\n    id SERIAL PRIMARY KEY,\n    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),\n    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()\n);\n\n-- @down\nDROP TABLE {tbl};"
        )),
        "add-column" => Ok(format!(
            "-- @up\nALTER TABLE {tbl} ADD COLUMN column_name TEXT;\n\n-- @down\nALTER TABLE {tbl} DROP COLUMN column_name;"
        )),
        "create-index" => Ok(format!(
            "-- @up\nCREATE INDEX idx_{tbl}_column ON {tbl}(column_name);\n\n-- @down\nDROP INDEX idx_{tbl}_column;"
        )),
        "create-enum" => Ok(
            "-- @up\nCREATE TYPE status AS ENUM ('active', 'inactive');\n\n-- @down(skip) Cannot remove enum type if in use".into(),
        ),
        "add-constraint" => Ok(format!(
            "-- @up\nALTER TABLE {tbl} ADD CONSTRAINT constraint_name CHECK (column > 0);\n\n-- @down\nALTER TABLE {tbl} DROP CONSTRAINT constraint_name;"
        )),
        "rename-column" => Ok(format!(
            "-- @up\nALTER TABLE {tbl} RENAME COLUMN old_name TO new_name;\n\n-- @down\nALTER TABLE {tbl} RENAME COLUMN new_name TO old_name;"
        )),
        "data" => Ok(format!(
            "-- @up\nINSERT INTO {tbl} (column) VALUES ('value');\n\n-- @down\nDELETE FROM {tbl} WHERE column = 'value';"
        )),
        other => Err(MigrateError::Config(format!("unknown template type: {other}"))),
    }
}

/// Write a migration file to the given directory.
pub fn write_migration(
    dir: &Path,
    filename: &str,
    content: &str,
) -> Result<std::path::PathBuf, MigrateError> {
    let path = dir.join(filename);
    std::fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_filename_format() {
        let name = generate_filename("create_users");
        assert!(name.ends_with("_create_users.sql"));
        assert_eq!(name.len(), "YYYYMMDD_HHMMSS_create_users.sql".len());
    }

    #[test]
    fn template_create_table() {
        let content = template_content("create-table", Some("users")).unwrap();
        assert!(content.contains("CREATE TABLE users"));
        assert!(content.contains("-- @up"));
        assert!(content.contains("-- @down"));
    }

    #[test]
    fn template_create_table_defaults_table_name() {
        let content = template_content("create-table", None).unwrap();
        assert!(content.contains("CREATE TABLE TABLE_NAME"));
    }

    #[test]
    fn template_add_column() {
        let content = template_content("add-column", Some("users")).unwrap();
        assert!(content.contains("ALTER TABLE users ADD COLUMN"));
    }

    #[test]
    fn template_create_index() {
        let content = template_content("create-index", Some("users")).unwrap();
        assert!(content.contains("CREATE INDEX idx_users_column ON users"));
        assert!(content.contains("DROP INDEX idx_users_column"));
    }

    #[test]
    fn template_create_enum() {
        let content = template_content("create-enum", None).unwrap();
        assert!(content.contains("CREATE TYPE status AS ENUM"));
        assert!(content.contains("-- @down(skip)"));
    }

    #[test]
    fn template_add_constraint() {
        let content = template_content("add-constraint", Some("orders")).unwrap();
        assert!(content.contains("ALTER TABLE orders ADD CONSTRAINT"));
        assert!(content.contains("ALTER TABLE orders DROP CONSTRAINT"));
    }

    #[test]
    fn template_rename_column() {
        let content = template_content("rename-column", Some("orders")).unwrap();
        assert!(content.contains("RENAME COLUMN old_name TO new_name"));
        assert!(content.contains("RENAME COLUMN new_name TO old_name"));
    }

    #[test]
    fn template_data() {
        let content = template_content("data", Some("orders")).unwrap();
        assert!(content.contains("INSERT INTO orders"));
        assert!(content.contains("DELETE FROM orders"));
    }

    #[test]
    fn template_unknown_type_errors() {
        let err = template_content("nonexistent", None).unwrap_err();
        assert!(err.to_string().contains("unknown template type"));
    }

    #[test]
    fn write_migration_creates_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path =
            write_migration(tmp.path(), "20260810_120000_test.sql", "-- @up\nSELECT 1;").unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "-- @up\nSELECT 1;");
    }

    #[test]
    fn write_migration_to_missing_dir_returns_io_error() {
        let err = write_migration(
            Path::new("/nonexistent/dir"),
            "20260810_120000_test.sql",
            "-- @up\nSELECT 1;",
        )
        .unwrap_err();
        assert!(matches!(err, MigrateError::Io(_)));
    }
}
