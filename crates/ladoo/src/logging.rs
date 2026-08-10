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

    let filter = if let Some(ref f) = config.filter {
        tracing_subscriber::EnvFilter::new(f)
    } else if let Ok(env) = std::env::var("RUST_LOG") {
        tracing_subscriber::EnvFilter::new(env)
    } else if let Some(ref level) = config.level {
        tracing_subscriber::EnvFilter::new(level)
    } else {
        tracing_subscriber::EnvFilter::new("info")
    };

    let env = crate::config::Environment::detect();

    // `try_init` (rather than `init`) avoids a panic if another thread
    // races us between the `has_been_set()` check above and this call.
    if env.is_dev() {
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
}
