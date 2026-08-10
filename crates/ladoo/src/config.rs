//! Application configuration and environment detection.
//!
//! The [`Environment`] enum detects whether the app is running in
//! development, staging, or production mode, reading from `LADOO_ENV`
//! or `APP_ENV` environment variables.

use std::fmt;

/// The application environment.
///
/// Detected from `LADOO_ENV` → `APP_ENV` → defaults to
/// [`Development`](Self::Development). Unknown values are treated as
/// development.
///
/// # Examples
///
/// ```
/// use ladoo::config::Environment;
///
/// // Without env vars set, defaults to Development
/// # std::env::remove_var("LADOO_ENV");
/// # std::env::remove_var("APP_ENV");
/// let env = Environment::detect();
/// assert!(env.is_dev());
/// assert_eq!(env.as_str(), "development");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Local development (the default).
    Development,
    /// Pre-production staging environment.
    Staging,
    /// Live production environment.
    Production,
}

impl Environment {
    /// Detect the environment from `LADOO_ENV` → `APP_ENV` → default `Development`.
    ///
    /// Unrecognized values are treated as [`Development`](Self::Development).
    pub fn detect() -> Self {
        match std::env::var("LADOO_ENV").or_else(|_| std::env::var("APP_ENV")) {
            Ok(val) => match val.as_str() {
                "production" => Self::Production,
                "staging" => Self::Staging,
                _ => Self::Development,
            },
            Err(_) => Self::Development,
        }
    }

    /// Returns `true` if this is [`Development`](Self::Development).
    pub fn is_dev(self) -> bool {
        matches!(self, Self::Development)
    }

    /// Returns the lowercase string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `Config` trait for typed configuration loading.
///
/// Implement this trait to load configuration from layered sources
/// (TOML files, environment variables, defaults). Use
/// `#[derive(Config)]` to generate the implementation automatically.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// #[derive(Config)]
/// struct AppConfig {
///     #[config(default = 3000)]
///     port: u16,
///     #[config(env = "DATABASE_URL")]
///     database_url: String,
/// }
///
/// let config = AppConfig::load().unwrap();
/// ```
#[cfg(feature = "config")]
pub trait Config: Sized + Send + Sync + 'static {
    /// Load configuration from layered sources.
    fn load() -> std::result::Result<Self, ConfigError>;
}

/// Errors that can occur when loading configuration.
///
/// Each variant includes enough context to produce a helpful error
/// message at startup. `ConfigError` implements [`IntoResponse`] for
/// cases where a config value is loaded lazily during a request (returns
/// 500 with detail in dev mode).
#[cfg(feature = "config")]
#[derive(Debug)]
pub enum ConfigError {
    /// A TOML file exists but contains invalid syntax.
    FileParseError {
        /// Path to the malformed file.
        path: std::path::PathBuf,
        /// The underlying parse error.
        source: toml::de::Error,
    },
    /// A TOML file could not be read (permissions, I/O error).
    /// Missing files are NOT errors — only files that exist but can't
    /// be read produce this variant.
    FileReadError {
        /// Path that failed to read.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// An environment variable was set but its value could not be parsed.
    EnvVarParse {
        /// The environment variable name.
        var: String,
        /// The raw value that failed to parse.
        value: String,
        /// The expected Rust type name.
        expected_type: &'static str,
    },
    /// A required field was not found in any configuration source.
    MissingField {
        /// The struct field name.
        field: &'static str,
        /// The expected Rust type name.
        expected_type: &'static str,
    },
    /// A TOML value could not be converted to the field's type.
    TomlTypeMismatch {
        /// The struct field name.
        field: &'static str,
        /// The expected Rust type name.
        expected_type: &'static str,
        /// String representation of the actual TOML value.
        actual: String,
    },
}

#[cfg(feature = "config")]
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileParseError { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
            Self::FileReadError { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::EnvVarParse {
                var,
                value,
                expected_type,
            } => {
                write!(f, "env var {var}={value:?} is not a valid {expected_type}")
            }
            Self::MissingField {
                field,
                expected_type,
            } => {
                write!(
                    f,
                    "missing required config field `{field}` ({expected_type})"
                )
            }
            Self::TomlTypeMismatch {
                field,
                expected_type,
                actual,
            } => {
                write!(
                    f,
                    "config field `{field}` expected {expected_type}, got {actual}"
                )
            }
        }
    }
}

#[cfg(feature = "config")]
impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileParseError { source, .. } => Some(source),
            Self::FileReadError { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(feature = "config")]
impl crate::response::IntoResponse for ConfigError {
    fn into_response(self) -> crate::response::Response {
        crate::error::Error::internal(format!("configuration error: {self}")).into_response()
    }
}

/// Load and merge TOML config files from the current working directory.
///
/// Reads `config/default.toml` then `config/{env}.toml`, merging
/// environment-specific values on top of defaults. Missing files are
/// silently skipped. Called by `#[derive(Config)]` generated code.
#[cfg(feature = "config")]
#[doc(hidden)]
pub fn load_toml_table() -> std::result::Result<toml::Table, ConfigError> {
    let cwd = std::env::current_dir().map_err(|e| ConfigError::FileReadError {
        path: std::path::PathBuf::from("."),
        source: e,
    })?;
    load_toml_from(cwd)
}

/// Load and merge TOML config files from the given base directory.
///
/// Reads `{base}/config/default.toml` then `{base}/config/{env}.toml`.
/// Missing files are silently skipped; files that exist but can't be
/// read or parsed produce an error.
#[cfg(feature = "config")]
pub fn load_toml_from(
    base: impl AsRef<std::path::Path>,
) -> std::result::Result<toml::Table, ConfigError> {
    let base = base.as_ref();
    let env = Environment::detect();
    let mut table = toml::Table::new();

    let default_path = base.join("config/default.toml");
    if default_path.exists() {
        let content = std::fs::read_to_string(&default_path).map_err(|e| {
            ConfigError::FileReadError {
                path: default_path.clone(),
                source: e,
            }
        })?;
        let parsed: toml::Table =
            toml::from_str(&content).map_err(|e| ConfigError::FileParseError {
                path: default_path,
                source: e,
            })?;
        table.extend(parsed);
    }

    let env_path = base.join(format!("config/{}.toml", env.as_str()));
    if env_path.exists() {
        let content =
            std::fs::read_to_string(&env_path).map_err(|e| ConfigError::FileReadError {
                path: env_path.clone(),
                source: e,
            })?;
        let parsed: toml::Table =
            toml::from_str(&content).map_err(|e| ConfigError::FileParseError {
                path: env_path,
                source: e,
            })?;
        table.extend(parsed);
    }

    Ok(table)
}

/// Convert a TOML value to a string for `FromStr` parsing.
///
/// Returns `None` for arrays, tables, and datetimes (unsupported in
/// flat config). The caller should produce a `TomlTypeMismatch` error.
#[cfg(feature = "config")]
fn toml_value_to_string(val: &toml::Value) -> Option<String> {
    match val {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Extract a typed value from a TOML table by field name.
///
/// Returns `Ok(None)` if the key is absent. Returns an error if the
/// key is present but cannot be converted to `T`.
#[cfg(feature = "config")]
#[doc(hidden)]
pub fn parse_toml_value<T: std::str::FromStr>(
    table: &toml::Table,
    field: &'static str,
    expected_type: &'static str,
) -> std::result::Result<Option<T>, ConfigError> {
    let Some(val) = table.get(field) else {
        return Ok(None);
    };
    let s = toml_value_to_string(val).ok_or_else(|| ConfigError::TomlTypeMismatch {
        field,
        expected_type,
        actual: format!("{val}"),
    })?;
    s.parse::<T>()
        .map(Some)
        .map_err(|_| ConfigError::TomlTypeMismatch {
            field,
            expected_type,
            actual: s,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn detect_defaults_to_development() {
        let _g = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        assert_eq!(Environment::detect(), Environment::Development);
    }

    #[test]
    fn detect_reads_ladoo_env_production() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        assert_eq!(Environment::detect(), Environment::Production);
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn detect_reads_ladoo_env_staging() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "staging");
        assert_eq!(Environment::detect(), Environment::Staging);
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn detect_reads_ladoo_env_development() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "development");
        assert_eq!(Environment::detect(), Environment::Development);
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn detect_falls_back_to_app_env() {
        let _g = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::set_var("APP_ENV", "production");
        assert_eq!(Environment::detect(), Environment::Production);
        std::env::remove_var("APP_ENV");
    }

    #[test]
    fn ladoo_env_takes_precedence_over_app_env() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "staging");
        std::env::set_var("APP_ENV", "production");
        assert_eq!(Environment::detect(), Environment::Staging);
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
    }

    #[test]
    fn detect_unknown_value_is_development() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "banana");
        assert_eq!(Environment::detect(), Environment::Development);
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn is_dev_true_for_development() {
        assert!(Environment::Development.is_dev());
    }

    #[test]
    fn is_dev_false_for_staging() {
        assert!(!Environment::Staging.is_dev());
    }

    #[test]
    fn is_dev_false_for_production() {
        assert!(!Environment::Production.is_dev());
    }

    #[test]
    fn as_str_returns_lowercase() {
        assert_eq!(Environment::Development.as_str(), "development");
        assert_eq!(Environment::Staging.as_str(), "staging");
        assert_eq!(Environment::Production.as_str(), "production");
    }

    #[test]
    fn display_delegates_to_as_str() {
        assert_eq!(format!("{}", Environment::Production), "production");
    }

    #[test]
    fn clone_and_copy() {
        let a = Environment::Development;
        let b = a;
        let c = a.clone();
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[cfg(feature = "config")]
    mod config_tests {
        use super::*;

        #[test]
        fn config_error_display_file_parse() {
            let err = ConfigError::FileParseError {
                path: std::path::PathBuf::from("config/default.toml"),
                source: "bad toml".parse::<toml::Table>().unwrap_err(),
            };
            let msg = format!("{err}");
            assert!(msg.contains("config/default.toml"));
        }

        #[test]
        fn config_error_display_file_read() {
            let err = ConfigError::FileReadError {
                path: std::path::PathBuf::from("config/missing.toml"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            };
            let msg = format!("{err}");
            assert!(msg.contains("config/missing.toml"));
        }

        #[test]
        fn config_error_display_env_var_parse() {
            let err = ConfigError::EnvVarParse {
                var: "PORT".to_string(),
                value: "abc".to_string(),
                expected_type: "u16",
            };
            let msg = format!("{err}");
            assert!(msg.contains("PORT"));
            assert!(msg.contains("abc"));
            assert!(msg.contains("u16"));
        }

        #[test]
        fn config_error_display_missing_field() {
            let err = ConfigError::MissingField {
                field: "database_url",
                expected_type: "String",
            };
            let msg = format!("{err}");
            assert!(msg.contains("database_url"));
        }

        #[test]
        fn config_error_display_toml_type_mismatch() {
            let err = ConfigError::TomlTypeMismatch {
                field: "port",
                expected_type: "u16",
                actual: "[1, 2, 3]".to_string(),
            };
            let msg = format!("{err}");
            assert!(msg.contains("port"));
            assert!(msg.contains("u16"));
        }

        #[test]
        fn config_error_is_std_error() {
            fn assert_error<T: std::error::Error>() {}
            assert_error::<ConfigError>();
        }

        #[test]
        fn config_error_is_send_sync() {
            fn assert_send_sync<T: Send + Sync>() {}
            assert_send_sync::<ConfigError>();
        }

        #[test]
        fn config_error_into_response_returns_500() {
            use crate::response::IntoResponse;
            let err = ConfigError::MissingField {
                field: "port",
                expected_type: "u16",
            };
            let resp = err.into_response();
            assert_eq!(resp.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
        }

        fn make_config_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
            let dir = tempfile::tempdir().unwrap();
            let config_dir = dir.path().join("config");
            std::fs::create_dir_all(&config_dir).unwrap();
            for (name, content) in files {
                std::fs::write(config_dir.join(name), content).unwrap();
            }
            dir
        }

        #[test]
        fn load_toml_from_empty_dir() {
            let dir = tempfile::tempdir().unwrap();
            let table = load_toml_from(dir.path()).unwrap();
            assert!(table.is_empty());
        }

        #[test]
        fn load_toml_from_reads_default() {
            let dir = make_config_dir(&[("default.toml", "port = 3000\nhost = \"localhost\"")]);
            let table = load_toml_from(dir.path()).unwrap();
            assert_eq!(table.get("port").unwrap().as_integer(), Some(3000));
            assert_eq!(table.get("host").unwrap().as_str(), Some("localhost"));
        }

        #[test]
        fn load_toml_from_env_overrides_default() {
            let _g = lock_env();
            std::env::set_var("LADOO_ENV", "production");
            let dir = make_config_dir(&[
                ("default.toml", "port = 3000\npool = 5"),
                ("production.toml", "pool = 20"),
            ]);
            let table = load_toml_from(dir.path()).unwrap();
            assert_eq!(table.get("port").unwrap().as_integer(), Some(3000));
            assert_eq!(table.get("pool").unwrap().as_integer(), Some(20));
            std::env::remove_var("LADOO_ENV");
        }

        #[test]
        fn load_toml_from_missing_env_file_is_ok() {
            let _g = lock_env();
            std::env::set_var("LADOO_ENV", "staging");
            let dir = make_config_dir(&[("default.toml", "port = 3000")]);
            let table = load_toml_from(dir.path()).unwrap();
            assert_eq!(table.get("port").unwrap().as_integer(), Some(3000));
            std::env::remove_var("LADOO_ENV");
        }

        #[test]
        fn load_toml_from_invalid_toml_returns_file_parse_error() {
            let dir = make_config_dir(&[("default.toml", "not valid { toml")]);
            let err = load_toml_from(dir.path()).unwrap_err();
            assert!(matches!(err, ConfigError::FileParseError { .. }));
        }

        #[test]
        fn parse_toml_value_extracts_integer() {
            let table: toml::Table = toml::from_str("port = 8080").unwrap();
            let val = parse_toml_value::<u16>(&table, "port", "u16").unwrap();
            assert_eq!(val, Some(8080));
        }

        #[test]
        fn parse_toml_value_extracts_string() {
            let table: toml::Table = toml::from_str("host = \"localhost\"").unwrap();
            let val = parse_toml_value::<String>(&table, "host", "String").unwrap();
            assert_eq!(val.as_deref(), Some("localhost"));
        }

        #[test]
        fn parse_toml_value_extracts_bool() {
            let table: toml::Table = toml::from_str("debug = true").unwrap();
            let val = parse_toml_value::<bool>(&table, "debug", "bool").unwrap();
            assert_eq!(val, Some(true));
        }

        #[test]
        fn parse_toml_value_extracts_float() {
            let table: toml::Table = toml::from_str("rate = 1.5").unwrap();
            let val = parse_toml_value::<f64>(&table, "rate", "f64").unwrap();
            assert!((val.unwrap() - 1.5).abs() < f64::EPSILON);
        }

        #[test]
        fn parse_toml_value_missing_key_returns_none() {
            let table: toml::Table = toml::from_str("port = 3000").unwrap();
            let val = parse_toml_value::<u16>(&table, "host", "u16").unwrap();
            assert_eq!(val, None);
        }

        #[test]
        fn parse_toml_value_wrong_type_returns_error() {
            let table: toml::Table = toml::from_str("port = \"not a number\"").unwrap();
            let err = parse_toml_value::<u16>(&table, "port", "u16").unwrap_err();
            assert!(matches!(err, ConfigError::TomlTypeMismatch { .. }));
        }

        #[test]
        fn parse_toml_value_array_returns_error() {
            let table: toml::Table = toml::from_str("port = [1, 2]").unwrap();
            let err = parse_toml_value::<u16>(&table, "port", "u16").unwrap_err();
            assert!(matches!(err, ConfigError::TomlTypeMismatch { .. }));
        }
    }
}
