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
}
