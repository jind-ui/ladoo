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
// Constructed by `App` builder methods in a later phase task; only
// exercised by tests until then.
#[allow(dead_code)]
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
}
