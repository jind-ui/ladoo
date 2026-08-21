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

use std::future::Future;
use std::pin::Pin;

use crate::context::Context;
use crate::error::Error;
use crate::middleware::{Middleware, Next};
use crate::response::Response;

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

/// Middleware that assigns a request ID to every request.
///
/// If the configured header is present on the incoming request, its
/// value is reused. Otherwise a UUID v4 is generated. The ID is
/// stored in per-request state as [`RequestId`] and added to the
/// response headers.
pub(crate) struct RequestIdMiddleware {
    header: String,
}

impl RequestIdMiddleware {
    /// Create a new request ID middleware with the given header name.
    pub(crate) fn new(header: String) -> Self {
        Self { header }
    }
}

impl Middleware for RequestIdMiddleware {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        let header = self.header.clone();
        Box::pin(async move {
            let mut ctx = ctx;

            let id = ctx
                .headers()
                .get(&header)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            ctx.provide(RequestId(id.clone()));

            let mut resp = next.run(ctx).await?;
            resp.set_header(&header, &id);
            Ok(resp)
        })
    }
}

/// Request logging middleware.
///
/// Creates a tracing span for each request containing the HTTP method,
/// path, and request ID. After the handler responds, emits an INFO
/// event with the status code and duration (or an ERROR event if the
/// chain returned an error).
///
/// Runs after [`RequestIdMiddleware`] in the chain so the request ID
/// is available in the span.
pub(crate) async fn request_logger(ctx: Context, next: Next) -> Result<Response, Error> {
    use tracing::Instrument;

    let method = ctx.method().to_string();
    let path = ctx.path().to_string();
    let request_id = ctx
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_default();

    let span = tracing::info_span!(
        "request",
        method = %method,
        path = %path,
        request_id = %request_id,
    );

    let start = std::time::Instant::now();
    let result = next.run(ctx).instrument(span.clone()).await;
    let duration_ms = start.elapsed().as_millis();

    match &result {
        Ok(resp) => {
            span.in_scope(|| {
                tracing::info!(
                    status = resp.status().as_u16(),
                    duration_ms = duration_ms as u64,
                    "{method} {path} \u{2190} {} ({duration_ms}ms)",
                    resp.status()
                );
            });
        }
        Err(err) => {
            span.in_scope(|| {
                tracing::error!(
                    duration_ms = duration_ms as u64,
                    error = %err,
                    "{method} {path} \u{2190} ERROR ({duration_ms}ms)"
                );
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::handler::IntoHandler;
    use crate::middleware;
    use crate::request::Request;
    use http::Method;
    use std::sync::Arc;
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
        std::env::set_var("LADOO_ENV", "development");
        let result = is_dev_format();
        std::env::remove_var("LADOO_ENV");
        assert!(result);
    }

    #[test]
    fn is_dev_format_false_by_default() {
        let _g = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        assert!(!is_dev_format());
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

    #[tokio::test]
    async fn request_id_middleware_generates_uuid_when_no_header() {
        let mw = RequestIdMiddleware::new("x-request-id".to_string());
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw_vec: Vec<Arc<dyn crate::middleware::Middleware>> = vec![Arc::new(mw)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let resp = middleware::run_middleware_chain(&mw_vec, handler, ctx)
            .await
            .unwrap();
        let id = resp
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(!id.is_empty());
        assert!(uuid::Uuid::parse_str(id).is_ok());
    }

    #[tokio::test]
    async fn request_id_middleware_uses_incoming_header() {
        let mw = RequestIdMiddleware::new("x-request-id".to_string());
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw_vec: Vec<Arc<dyn crate::middleware::Middleware>> = vec![Arc::new(mw)];
        let mut headers = http::HeaderMap::new();
        headers.insert("x-request-id", "custom-123".parse().unwrap());
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let ctx = Context::new(req);
        let resp = middleware::run_middleware_chain(&mw_vec, handler, ctx)
            .await
            .unwrap();
        let id = resp
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(id, "custom-123");
    }

    #[tokio::test]
    async fn request_id_middleware_makes_state_extractable() {
        let mw = RequestIdMiddleware::new("x-request-id".to_string());
        let handler: Arc<dyn crate::handler::Handler> =
            (|id: crate::state::State<RequestId>| format!("id: {}", id.0))
                .into_handler()
                .into();
        let mw_vec: Vec<Arc<dyn crate::middleware::Middleware>> = vec![Arc::new(mw)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let resp = middleware::run_middleware_chain(&mw_vec, handler, ctx)
            .await
            .unwrap();
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.starts_with("id: "));
    }

    #[tokio::test]
    async fn request_id_middleware_custom_header() {
        let mw = RequestIdMiddleware::new("x-trace-id".to_string());
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw_vec: Vec<Arc<dyn crate::middleware::Middleware>> = vec![Arc::new(mw)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let resp = middleware::run_middleware_chain(&mw_vec, handler, ctx)
            .await
            .unwrap();
        assert!(resp.headers().contains_key("x-trace-id"));
        assert!(!resp.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn request_logger_passes_through() {
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn crate::middleware::Middleware>> =
            vec![Arc::new(request_logger as fn(Context, Next) -> _)];
        let req = Request::test(Method::GET, "/test");
        let mut ctx = Context::new(req);
        ctx.provide(RequestId("test-id".to_string()));
        let resp = middleware::run_middleware_chain(&mw, handler, ctx)
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"ok");
    }

    #[tokio::test]
    async fn request_logger_works_without_request_id() {
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn crate::middleware::Middleware>> =
            vec![Arc::new(request_logger as fn(Context, Next) -> _)];
        let ctx = Context::new(Request::test(Method::POST, "/users"));
        let resp = middleware::run_middleware_chain(&mw, handler, ctx)
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
    }

    #[tokio::test]
    async fn request_id_and_logger_combined() {
        let id_mw = RequestIdMiddleware::new("x-request-id".to_string());
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn crate::middleware::Middleware>> = vec![
            Arc::new(id_mw),
            Arc::new(request_logger as fn(Context, Next) -> _),
        ];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let resp = middleware::run_middleware_chain(&mw, handler, ctx)
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert!(resp.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn request_logger_reports_error_status() {
        async fn blocker(_ctx: Context, _next: Next) -> Result<Response, Error> {
            Err(crate::error::Error::internal("boom"))
        }
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();
        let mw: Vec<Arc<dyn crate::middleware::Middleware>> = vec![
            Arc::new(request_logger as fn(Context, Next) -> _),
            Arc::new(blocker as fn(Context, Next) -> _),
        ];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = middleware::run_middleware_chain(&mw, handler, ctx).await;
        assert!(result.is_err());
    }
}
