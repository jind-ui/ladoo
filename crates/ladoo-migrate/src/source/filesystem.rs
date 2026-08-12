//! Filesystem-backed migration source.
//!
//! Reads `.sql` files from a directory, parsing each through the
//! [`parser`](super::parser). Versioned migrations live in the root
//! directory; repeatable migrations live in a `repeatable/` subdirectory.

use std::fs;
use std::path::{Path, PathBuf};

use super::parser::parse_migration_file;
use super::MigrationSource;
use crate::migration::Migration;
use crate::MigrateError;

/// Loads migrations from the filesystem.
///
/// Versioned migrations are read from the root `dir` and sorted by
/// version. Repeatable migrations are read from `dir/repeatable/` and
/// sorted alphabetically by filename.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo_migrate::source::FilesystemSource;
/// use ladoo_migrate::source::MigrationSource;
///
/// let source = FilesystemSource::new("migrations");
/// let pending = source.load_versioned()?;
/// ```
pub struct FilesystemSource {
    /// Root directory containing migration files.
    dir: PathBuf,
}

impl FilesystemSource {
    /// Create a new filesystem source reading from the given directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Returns the configured directory path.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn read_sql_files(&self, dir: &Path) -> Result<Vec<(String, String)>, MigrateError> {
        if !dir.exists() {
            return Err(MigrateError::Config(format!(
                "migrations directory not found: {}",
                dir.display()
            )));
        }

        let mut files = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("sql") {
                let filename = filename_str(&path)?;
                let content = fs::read_to_string(&path)?;
                files.push((filename, content));
            }
        }
        Ok(files)
    }
}

/// Returns a path's filename as an owned `String`.
///
/// Returns `MigrateError::Parse` if the path has no filename component or
/// the filename is not valid UTF-8.
fn filename_str(path: &Path) -> Result<String, MigrateError> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| MigrateError::Parse {
            file: path.display().to_string(),
            message: "invalid filename encoding".into(),
        })
}

impl MigrationSource for FilesystemSource {
    fn load_versioned(&self) -> Result<Vec<Migration>, MigrateError> {
        let files = self.read_sql_files(&self.dir)?;
        let mut migrations = Vec::new();
        for (filename, content) in &files {
            migrations.push(parse_migration_file(filename, content)?);
        }
        migrations.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(migrations)
    }

    fn load_repeatable(&self) -> Result<Vec<Migration>, MigrateError> {
        let rep_dir = self.dir.join("repeatable");
        if !rep_dir.exists() {
            return Ok(Vec::new());
        }
        let files = self.read_sql_files(&rep_dir)?;
        let mut migrations = Vec::new();
        for (filename, content) in &files {
            migrations.push(parse_migration_file(filename, content)?);
        }
        // Filename is `{version}_{name}.sql`, so sorting by version then
        // name reproduces alphabetical filename order without needing to
        // retain the original filename on `Migration`.
        migrations.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.name.cmp(&b.name)));
        Ok(migrations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_migration(dir: &Path, filename: &str, content: &str) {
        fs::write(dir.join(filename), content).unwrap();
    }

    #[test]
    fn loads_versioned_migrations_sorted() {
        let tmp = TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260810_120000_second.sql",
            "-- @up\nCREATE TABLE b (id INT);",
        );
        write_migration(
            tmp.path(),
            "20260810_100000_first.sql",
            "-- @up\nCREATE TABLE a (id INT);",
        );

        let source = FilesystemSource::new(tmp.path());
        let migrations = source.load_versioned().unwrap();

        assert_eq!(migrations.len(), 2);
        assert_eq!(migrations[0].version, "20260810_100000");
        assert_eq!(migrations[1].version, "20260810_120000");
    }

    #[test]
    fn loads_repeatable_migrations_alphabetically() {
        let tmp = TempDir::new().unwrap();
        let rep_dir = tmp.path().join("repeatable");
        fs::create_dir(&rep_dir).unwrap();

        // Names are chosen so that sorting by `name` alone would give the
        // opposite order to sorting by filename (version + name): "zzz"
        // sorts after "aaa" by name, but its file has the earlier version,
        // so it must come first when ordering by filename.
        write_migration(
            &rep_dir,
            "20260810_200000_aaa.sql",
            "-- @up\n-- @repeatable\nCREATE OR REPLACE VIEW aaa AS SELECT 1;",
        );
        write_migration(
            &rep_dir,
            "20260810_100000_zzz.sql",
            "-- @up\n-- @repeatable\nGRANT SELECT ON users TO reader;",
        );

        let source = FilesystemSource::new(tmp.path());
        let migrations = source.load_repeatable().unwrap();

        assert_eq!(migrations.len(), 2);
        // Filename order (20260810_100000_zzz.sql < 20260810_200000_aaa.sql),
        // not name order (which would put "aaa" first).
        assert_eq!(migrations[0].name, "zzz");
        assert_eq!(migrations[1].name, "aaa");
    }

    #[test]
    fn returns_empty_when_no_repeatable_dir() {
        let tmp = TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260810_120000_init.sql",
            "-- @up\nSELECT 1;",
        );

        let source = FilesystemSource::new(tmp.path());
        assert!(source.load_repeatable().unwrap().is_empty());
    }

    #[test]
    fn error_when_directory_missing() {
        let source = FilesystemSource::new("/nonexistent/path");
        let err = source.load_versioned().unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn ignores_non_sql_files() {
        let tmp = TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260810_120000_init.sql",
            "-- @up\nSELECT 1;",
        );
        fs::write(tmp.path().join("README.md"), "# Migrations").unwrap();
        fs::write(tmp.path().join(".gitkeep"), "").unwrap();

        let source = FilesystemSource::new(tmp.path());
        let migrations = source.load_versioned().unwrap();
        assert_eq!(migrations.len(), 1);
    }

    #[test]
    fn parse_error_propagated() {
        let tmp = TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260810_120000_bad.sql",
            "no markers here",
        );

        let source = FilesystemSource::new(tmp.path());
        let err = source.load_versioned().unwrap_err();
        assert!(err.to_string().contains("missing -- @up"));
    }

    #[test]
    fn dir_accessor() {
        let source = FilesystemSource::new("/some/path");
        assert_eq!(source.dir(), Path::new("/some/path"));
    }

    #[test]
    fn filename_str_returns_valid_utf8_filename() {
        let path = Path::new("/migrations/20260810_120000_init.sql");
        assert_eq!(filename_str(path).unwrap(), "20260810_120000_init.sql");
    }

    // Exercises the non-UTF-8 branch via an in-memory `PathBuf` — no real
    // file is created, since macOS (APFS) rejects invalid-UTF-8 filenames
    // at the syscall level and this would otherwise be untestable there.
    #[cfg(unix)]
    #[test]
    fn filename_str_rejects_non_utf8_filename() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let invalid = OsStr::from_bytes(b"\xFF\xFE_bad.sql");
        let path = Path::new("/migrations").join(invalid);

        let err = filename_str(&path).unwrap_err();
        assert!(err.to_string().contains("invalid filename encoding"));
    }
}
