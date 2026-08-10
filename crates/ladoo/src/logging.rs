//! Structured logging and request tracing.
//!
//! Provides automatic request logging with request ID propagation.
//! Dev mode uses pretty-printed colored output, prod mode uses JSON
//! for log aggregators. Every request gets a UUID request ID,
//! propagated via the `X-Request-Id` header.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! // Zero-config — logging works out of the box
//! App::new()
//!     .get("/", |_: Request| "hello")
//!     .run("0.0.0.0:3000");
//! // Dev:  INFO request{method=GET path=/ request_id=abc-123} ← 200 OK (1ms)
//! // Prod: {"timestamp":"...","level":"info","method":"GET","path":"/","status":200}
//! ```

/// Configuration for the logging system.
///
/// Stored internally in [`App`](crate::app::App) and used to
/// configure the tracing subscriber and built-in middleware.
/// Users configure it via builder methods on `App`.
pub(crate) struct LoggingConfig {
    pub(crate) level: Option<String>,
    pub(crate) filter: Option<String>,
    pub(crate) request_logging: bool,
    pub(crate) request_id_header: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: None,
            filter: None,
            request_logging: true,
            request_id_header: "x-request-id".to_string(),
        }
    }
}

/// Resolve the tracing filter from config and environment.
///
/// Precedence, highest to lowest: [`LoggingConfig::filter`] → `RUST_LOG`
/// → [`LoggingConfig::level`] → `"info"`.
pub(crate) fn resolve_filter(config: &LoggingConfig) -> tracing_subscriber::EnvFilter {
    if let Some(ref f) = config.filter {
        tracing_subscriber::EnvFilter::new(f)
    } else if let Ok(env) = std::env::var("RUST_LOG") {
        tracing_subscriber::EnvFilter::new(env)
    } else if let Some(ref level) = config.level {
        tracing_subscriber::EnvFilter::new(level)
    } else {
        tracing_subscriber::EnvFilter::new("info")
    }
}

/// Returns `true` when the detected environment should use pretty,
/// human-readable output rather than JSON.
pub(crate) fn is_dev_format() -> bool {
    crate::config::Environment::detect().is_dev()
}

/// Initialize the global tracing subscriber.
///
/// Picks format based on the detected environment: pretty-printed
/// with colors in development, JSON in staging/production. Respects
/// `RUST_LOG` and the app's configured level/filter.
///
/// If a subscriber has already been set (e.g., by the user calling
/// `tracing_subscriber::init()` before `App::run()`), this is a no-op.
pub(crate) fn init_subscriber(config: &LoggingConfig) {
    if tracing::dispatcher::has_been_set() {
        return;
    }

    let filter = resolve_filter(config);

    // `try_init` (rather than `init`) avoids a panic if another thread
    // races us between the `has_been_set()` check above and this call.
    if is_dev_format() {
        let _ = tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(filter)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .try_init();
    }
}

/// The request ID for the current request.
///
/// Automatically generated (UUID v4) or extracted from the incoming
/// request ID header (default: `X-Request-Id`). Access in handlers
/// via `State<RequestId>`:
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// async fn handler(id: State<RequestId>) -> String {
///     format!("Your request: {}", id.0)
/// }
/// ```
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `resolve_filter` reads `RUST_LOG` and `is_dev_format` reads
    // `LADOO_ENV`/`APP_ENV` (via `Environment::detect`) — both process
    // globals. Every test in this module that touches them goes through
    // this lock so they don't race each other, matching the pattern used
    // in `config.rs` and `error.rs` for the same env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn logging_config_default_level_is_none() {
        let config = LoggingConfig::default();
        assert!(config.level.is_none());
    }

    #[test]
    fn logging_config_default_filter_is_none() {
        let config = LoggingConfig::default();
        assert!(config.filter.is_none());
    }

    #[test]
    fn logging_config_default_request_logging_is_true() {
        let config = LoggingConfig::default();
        assert!(config.request_logging);
    }

    #[test]
    fn logging_config_default_header_is_x_request_id() {
        let config = LoggingConfig::default();
        assert_eq!(config.request_id_header, "x-request-id");
    }

    #[test]
    fn request_id_display() {
        let id = RequestId("abc-123".to_string());
        assert_eq!(id.to_string(), "abc-123");
    }

    #[test]
    fn request_id_clone() {
        let id = RequestId("abc-123".to_string());
        let cloned = id.clone();
        assert_eq!(id.0, cloned.0);
    }

    #[test]
    fn request_id_debug() {
        let id = RequestId("abc-123".to_string());
        let debug = format!("{id:?}");
        assert!(debug.contains("abc-123"));
    }

    #[test]
    fn init_subscriber_does_not_panic() {
        let config = LoggingConfig::default();
        init_subscriber(&config);
    }

    #[test]
    fn init_subscriber_is_noop_when_already_set() {
        let config = LoggingConfig::default();
        // First call sets the dispatcher (or is a no-op if another test
        // already did). Second call is guaranteed to hit the
        // `has_been_set()` early-return branch either way.
        init_subscriber(&config);
        init_subscriber(&config);
        assert!(tracing::dispatcher::has_been_set());
    }

    #[test]
    fn resolve_filter_uses_config_filter_when_set() {
        let _g = lock_env();
        std::env::remove_var("RUST_LOG");
        let config = LoggingConfig {
            filter: Some("my_app=debug,sqlx=warn".to_string()),
            ..LoggingConfig::default()
        };
        assert_eq!(
            resolve_filter(&config).to_string(),
            "my_app=debug,sqlx=warn"
        );
    }

    #[test]
    fn resolve_filter_uses_rust_log_when_filter_unset() {
        let _g = lock_env();
        std::env::set_var("RUST_LOG", "warn");
        let config = LoggingConfig::default();
        let result = resolve_filter(&config).to_string();
        std::env::remove_var("RUST_LOG");
        assert_eq!(result, "warn");
    }

    #[test]
    fn resolve_filter_uses_config_level_when_filter_and_rust_log_unset() {
        let _g = lock_env();
        std::env::remove_var("RUST_LOG");
        let config = LoggingConfig {
            level: Some("debug".to_string()),
            ..LoggingConfig::default()
        };
        assert_eq!(resolve_filter(&config).to_string(), "debug");
    }

    #[test]
    fn resolve_filter_defaults_to_info() {
        let _g = lock_env();
        std::env::remove_var("RUST_LOG");
        let config = LoggingConfig::default();
        assert_eq!(resolve_filter(&config).to_string(), "info");
    }

    #[test]
    fn resolve_filter_config_filter_takes_precedence_over_rust_log() {
        let _g = lock_env();
        std::env::set_var("RUST_LOG", "error");
        let config = LoggingConfig {
            filter: Some("debug".to_string()),
            ..LoggingConfig::default()
        };
        let result = resolve_filter(&config).to_string();
        std::env::remove_var("RUST_LOG");
        assert_eq!(result, "debug");
    }

    #[test]
    fn is_dev_format_true_in_development() {
        let _g = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        assert!(is_dev_format());
    }

    #[test]
    fn is_dev_format_false_in_production() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        let result = is_dev_format();
        std::env::remove_var("LADOO_ENV");
        assert!(!result);
    }

    #[test]
    fn is_dev_format_false_in_staging() {
        let _g = lock_env();
        std::env::set_var("LADOO_ENV", "staging");
        let result = is_dev_format();
        std::env::remove_var("LADOO_ENV");
        assert!(!result);
    }
}
