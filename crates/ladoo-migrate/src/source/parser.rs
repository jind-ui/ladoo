//! SQL migration file parser.
//!
//! Parses `.sql` files into [`Migration`] structs. Handles all directives:
//! `@up`, `@down`, `@down(skip)`, `@no-transaction`, `@requires`, `@repeatable`.
//!
//! # File Format
//!
//! ```sql
//! -- File-level comments are ignored
//! -- Everything before @up is skipped
//!
//! -- @up
//! -- @no-transaction
//! -- @requires 20260810_100000
//! CREATE INDEX CONCURRENTLY idx_orders_email ON orders(email);
//!
//! -- @down(skip) Cannot reverse concurrent index creation
//! ```
//!
//! # Filename Format
//!
//! ```text
//! YYYYMMDD_HHMMSS_descriptive_name.sql
//! ^version^       ^name^
//! ```

use crate::checksum::compute_checksum;
use crate::migration::Migration;
use crate::MigrateError;

/// Parse a migration filename into `(version, name)`.
///
/// Expects format `YYYYMMDD_HHMMSS_descriptive_name.sql`.
/// Returns `MigrateError::Parse` if the filename doesn't match — missing
/// `.sql` extension, too short, malformed version, or an empty name.
///
/// # Examples
///
/// ```
/// use ladoo_migrate::source::parser::parse_filename;
///
/// let (version, name) = parse_filename("20260810_120000_create_users.sql").unwrap();
/// assert_eq!(version, "20260810_120000");
/// assert_eq!(name, "create_users");
/// ```
pub fn parse_filename(filename: &str) -> Result<(String, String), MigrateError> {
    let stem = filename
        .strip_suffix(".sql")
        .ok_or_else(|| MigrateError::Parse {
            file: filename.into(),
            message: "migration file must have .sql extension".into(),
        })?;

    let format_err = || MigrateError::Parse {
        file: filename.into(),
        message: "expected format YYYYMMDD_HHMMSS_name.sql".into(),
    };

    // Version is YYYYMMDD_HHMMSS = 15 characters, followed by an
    // underscore separator. `get` (rather than indexing) avoids panics
    // on filenames that are too short or contain multi-byte characters
    // that don't fall on a UTF-8 char boundary.
    if stem.as_bytes().get(15) != Some(&b'_') {
        return Err(format_err());
    }
    let version = stem.get(..15).ok_or_else(format_err)?;
    let name = stem.get(16..).ok_or_else(format_err)?;

    // Version must be exactly YYYYMMDD_HHMMSS: 8 digits, an underscore,
    // then 6 digits. Byte indexing here is safe regardless of UTF-8
    // boundaries — it never panics, unlike string slicing.
    let vb = version.as_bytes();
    let valid_version = vb.len() == 15
        && vb[..8].iter().all(u8::is_ascii_digit)
        && vb[8] == b'_'
        && vb[9..15].iter().all(u8::is_ascii_digit);
    if !valid_version {
        return Err(MigrateError::Parse {
            file: filename.into(),
            message: "version must be YYYYMMDD_HHMMSS (14 digits)".into(),
        });
    }

    if name.is_empty() {
        return Err(MigrateError::Parse {
            file: filename.into(),
            message: "migration name cannot be empty".into(),
        });
    }

    Ok((version.to_string(), name.to_string()))
}

/// Parse a migration file's content into a [`Migration`].
///
/// The filename provides the version and name; the content is parsed
/// for `@up`, `@down`, and directive markers. Everything before the
/// first `-- @up` line is ignored. The checksum is computed from the
/// `@up` block content only (after directives, before `@down`).
///
/// # Errors
///
/// Returns `MigrateError::Parse` if:
/// - The filename doesn't match the expected `YYYYMMDD_HHMMSS_name.sql` format
/// - The file has no `-- @up` marker
pub fn parse_migration_file(filename: &str, content: &str) -> Result<Migration, MigrateError> {
    let (version, name) = parse_filename(filename)?;

    let mut in_up = false;
    let mut in_down = false;
    let mut past_directives = false;
    let mut found_up = false;

    let mut up_lines: Vec<&str> = Vec::new();
    let mut down_lines: Vec<&str> = Vec::new();
    let mut no_transaction = false;
    let mut requires: Vec<String> = Vec::new();
    let mut repeatable = false;
    let mut down_skip_reason: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "-- @up" {
            found_up = true;
            in_up = true;
            in_down = false;
            past_directives = false;
            continue;
        }

        if trimmed == "-- @down" {
            in_up = false;
            in_down = true;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("-- @down(skip)") {
            in_up = false;
            in_down = false;
            down_skip_reason = Some(rest.trim().to_string());
            continue;
        }

        if in_up && !past_directives {
            if trimmed == "-- @no-transaction" {
                no_transaction = true;
                continue;
            }
            if let Some(dep) = trimmed.strip_prefix("-- @requires ") {
                requires.push(dep.trim().to_string());
                continue;
            }
            if trimmed == "-- @repeatable" {
                repeatable = true;
                continue;
            }
            // Any non-empty, non-comment line means we're past directives
            // and into real SQL — later lines are no longer inspected for
            // directive syntax, even if they happen to look like one.
            if !trimmed.is_empty() && !trimmed.starts_with("--") {
                past_directives = true;
            }
        }

        if in_up {
            up_lines.push(line);
        } else if in_down {
            down_lines.push(line);
        }
    }

    if !found_up {
        return Err(MigrateError::Parse {
            file: filename.into(),
            message: "missing -- @up directive".into(),
        });
    }

    let up_sql = up_lines.join("\n").trim().to_string();
    let checksum = compute_checksum(&up_sql);

    let down_sql = if down_lines.is_empty() {
        None
    } else {
        let joined = down_lines.join("\n").trim().to_string();
        if joined.is_empty() {
            None
        } else {
            Some(joined)
        }
    };

    Ok(Migration {
        version,
        name,
        up_sql,
        down_sql,
        down_skip_reason,
        checksum,
        no_transaction,
        requires,
        repeatable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Filename parsing tests ---

    #[test]
    fn parse_valid_filename() {
        let (v, n) = parse_filename("20260810_120000_create_users.sql").unwrap();
        assert_eq!(v, "20260810_120000");
        assert_eq!(n, "create_users");
    }

    #[test]
    fn parse_filename_with_multiple_underscores() {
        let (v, n) = parse_filename("20260810_120000_add_email_to_users.sql").unwrap();
        assert_eq!(v, "20260810_120000");
        assert_eq!(n, "add_email_to_users");
    }

    #[test]
    fn parse_filename_missing_extension() {
        let err = parse_filename("20260810_120000_create_users").unwrap_err();
        assert!(err.to_string().contains(".sql extension"));
    }

    #[test]
    fn parse_filename_too_short() {
        let err = parse_filename("short.sql").unwrap_err();
        assert!(err.to_string().contains("YYYYMMDD_HHMMSS"));
    }

    #[test]
    fn parse_filename_empty_name() {
        let err = parse_filename("20260810_120000_.sql").unwrap_err();
        assert!(err.to_string().contains("name cannot be empty"));
    }

    #[test]
    fn parse_filename_invalid_version_chars() {
        let err = parse_filename("2026ABCD_120000_test.sql").unwrap_err();
        assert!(err.to_string().contains("14 digits"));
    }

    #[test]
    fn parse_filename_missing_second_underscore() {
        let err = parse_filename("20260810X120000_test.sql").unwrap_err();
        assert!(err.to_string().contains("YYYYMMDD_HHMMSS"));
    }

    #[test]
    fn parse_filename_multibyte_chars_do_not_panic() {
        // Four 4-byte emoji place byte offset 15 mid-character, not on a
        // UTF-8 char boundary. Must return an error, not panic.
        let err = parse_filename("\u{1f980}\u{1f980}\u{1f980}\u{1f980}_120000_test.sql")
            .unwrap_err();
        assert!(err.to_string().contains("YYYYMMDD_HHMMSS"));
    }

    // --- File content parsing tests ---

    #[test]
    fn parse_basic_up_and_down() {
        let content = "\
-- @up
CREATE TABLE users (id INT);

-- @down
DROP TABLE users;";

        let m = parse_migration_file("20260810_120000_create_users.sql", content).unwrap();
        assert_eq!(m.version, "20260810_120000");
        assert_eq!(m.name, "create_users");
        assert_eq!(m.up_sql, "CREATE TABLE users (id INT);");
        assert_eq!(m.down_sql.as_deref(), Some("DROP TABLE users;"));
        assert!(!m.no_transaction);
        assert!(!m.repeatable);
        assert!(m.requires.is_empty());
        assert!(m.down_skip_reason.is_none());
    }

    #[test]
    fn parse_with_file_level_comments() {
        let content = "\
-- This is a file-level comment
-- Author: neel
-- Created for user management

-- @up
CREATE TABLE users (id INT);

-- @down
DROP TABLE users;";

        let m = parse_migration_file("20260810_120000_create_users.sql", content).unwrap();
        assert_eq!(m.up_sql, "CREATE TABLE users (id INT);");
    }

    #[test]
    fn parse_no_transaction_directive() {
        let content = "\
-- @up
-- @no-transaction
CREATE INDEX CONCURRENTLY idx_email ON users(email);

-- @down(skip) Cannot reverse concurrent index creation";

        let m = parse_migration_file("20260810_120000_add_index.sql", content).unwrap();
        assert!(m.no_transaction);
        assert!(m.down_sql.is_none());
        assert_eq!(
            m.down_skip_reason.as_deref(),
            Some("Cannot reverse concurrent index creation")
        );
    }

    #[test]
    fn parse_requires_directive() {
        let content = "\
-- @up
-- @requires 20260810_100000
CREATE TABLE orders (id INT, user_id INT REFERENCES users(id));

-- @down
DROP TABLE orders;";

        let m = parse_migration_file("20260810_120000_create_orders.sql", content).unwrap();
        assert_eq!(m.requires, vec!["20260810_100000"]);
    }

    #[test]
    fn parse_multiple_requires() {
        let content = "\
-- @up
-- @requires 20260810_100000
-- @requires 20260810_110000
ALTER TABLE orders ADD COLUMN product_id INT;

-- @down
ALTER TABLE orders DROP COLUMN product_id;";

        let m = parse_migration_file("20260810_120000_add_product.sql", content).unwrap();
        assert_eq!(m.requires, vec!["20260810_100000", "20260810_110000"]);
    }

    #[test]
    fn parse_repeatable_directive() {
        let content = "\
-- @up
-- @repeatable
CREATE OR REPLACE FUNCTION now_utc() RETURNS TIMESTAMPTZ AS $$
  SELECT NOW() AT TIME ZONE 'UTC';
$$ LANGUAGE SQL;";

        let m = parse_migration_file("20260810_120000_now_utc.sql", content).unwrap();
        assert!(m.repeatable);
    }

    #[test]
    fn parse_down_skip_with_reason() {
        let content = "\
-- @up
ALTER TYPE status ADD VALUE 'archived';

-- @down(skip) Cannot remove enum values in PostgreSQL";

        let m = parse_migration_file("20260810_120000_add_enum.sql", content).unwrap();
        assert!(m.down_sql.is_none());
        assert_eq!(
            m.down_skip_reason.as_deref(),
            Some("Cannot remove enum values in PostgreSQL")
        );
    }

    #[test]
    fn parse_empty_up_block() {
        let content = "\
-- @up

-- @down
DROP TABLE IF EXISTS temp;";

        let m = parse_migration_file("20260810_120000_empty.sql", content).unwrap();
        assert!(m.up_sql.is_empty());
    }

    #[test]
    fn parse_up_only_no_down() {
        let content = "\
-- @up
CREATE TABLE users (id INT);";

        let m = parse_migration_file("20260810_120000_create_users.sql", content).unwrap();
        assert_eq!(m.up_sql, "CREATE TABLE users (id INT);");
        assert!(m.down_sql.is_none());
        assert!(m.down_skip_reason.is_none());
    }

    #[test]
    fn parse_missing_up_directive() {
        let content = "CREATE TABLE users (id INT);";
        let err =
            parse_migration_file("20260810_120000_create_users.sql", content).unwrap_err();
        assert!(err.to_string().contains("missing -- @up"));
    }

    #[test]
    fn parse_missing_up_directive_propagates_bad_filename_first() {
        // A malformed filename should fail before content is even inspected.
        let err = parse_migration_file("not-a-migration.sql", "SELECT 1;").unwrap_err();
        assert!(err.to_string().contains("YYYYMMDD_HHMMSS"));
    }

    #[test]
    fn parse_multiline_up_sql() {
        let content = "\
-- @up
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT UNIQUE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- @down
DROP TABLE users;";

        let m = parse_migration_file("20260810_120000_create_users.sql", content).unwrap();
        assert!(m.up_sql.contains("id SERIAL PRIMARY KEY"));
        assert!(m.up_sql.contains("created_at TIMESTAMPTZ"));
    }

    #[test]
    fn checksum_covers_up_block_only() {
        let content_a = "\
-- @up
CREATE TABLE users (id INT);

-- @down
DROP TABLE users;";

        let content_b = "\
-- @up
CREATE TABLE users (id INT);

-- @down
DROP TABLE users CASCADE;";

        let m_a = parse_migration_file("20260810_120000_test.sql", content_a).unwrap();
        let m_b = parse_migration_file("20260810_120000_test.sql", content_b).unwrap();
        assert_eq!(m_a.checksum, m_b.checksum);
    }

    #[test]
    fn checksum_changes_with_up_sql() {
        let content_a = "\
-- @up
CREATE TABLE users (id INT);";

        let content_b = "\
-- @up
CREATE TABLE users (id BIGINT);";

        let m_a = parse_migration_file("20260810_120000_test.sql", content_a).unwrap();
        let m_b = parse_migration_file("20260810_120000_test.sql", content_b).unwrap();
        assert_ne!(m_a.checksum, m_b.checksum);
    }

    #[test]
    fn parse_all_directives_combined() {
        let content = "\
-- This migration adds an index for performance
-- @up
-- @no-transaction
-- @requires 20260810_100000
CREATE INDEX CONCURRENTLY idx_orders_email ON orders(email);

-- @down(skip) Cannot reverse concurrent index creation";

        let m = parse_migration_file("20260810_120000_add_index.sql", content).unwrap();
        assert!(m.no_transaction);
        assert_eq!(m.requires, vec!["20260810_100000"]);
        assert!(!m.repeatable);
        assert!(m.down_sql.is_none());
        assert!(m.down_skip_reason.is_some());
        assert!(!m.up_sql.contains("@no-transaction"));
        assert!(!m.up_sql.contains("@requires"));
    }

    #[test]
    fn checksum_excludes_directives() {
        let content_with = "\
-- @up
-- @no-transaction
CREATE INDEX idx ON t(c);";

        let content_without = "\
-- @up
CREATE INDEX idx ON t(c);";

        let m_with = parse_migration_file("20260810_120000_test.sql", content_with).unwrap();
        let m_without =
            parse_migration_file("20260810_120000_test.sql", content_without).unwrap();
        assert_eq!(m_with.checksum, m_without.checksum);
    }

    #[test]
    fn directive_like_line_after_sql_is_treated_as_a_plain_comment() {
        // Once we're past the directive-only prefix of the @up block,
        // later lines are ordinary SQL (or SQL comments) — even if they
        // happen to look like a directive.
        let content = "\
-- @up
CREATE INDEX idx ON t(c);
-- @no-transaction
DROP INDEX idx;";

        let m = parse_migration_file("20260810_120000_test.sql", content).unwrap();
        assert!(!m.no_transaction);
        assert!(m.up_sql.contains("-- @no-transaction"));
    }

    #[test]
    fn down_block_with_only_blank_lines_is_none() {
        let content = "\
-- @up
CREATE TABLE t (id INT);

-- @down

";

        let m = parse_migration_file("20260810_120000_test.sql", content).unwrap();
        assert!(m.down_sql.is_none());
    }

    #[test]
    fn repeatable_migration_has_no_down_block() {
        let content = "\
-- @up
-- @repeatable
GRANT SELECT ON users TO readonly;";

        let m = parse_migration_file("20260810_120000_grants.sql", content).unwrap();
        assert!(m.repeatable);
        assert!(m.down_sql.is_none());
        assert!(m.down_skip_reason.is_none());
    }
}
