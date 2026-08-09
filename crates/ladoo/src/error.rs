//! Unified error type for the Ladoo framework.
//!
//! Provides three levels of error control:
//!
//! 1. **Auto-500** — use `?` on any `std::error::Error`; it becomes a 500.
//! 2. **Controlled status** — use constructors like [`Error::not_found`].
//! 3. **Domain errors** — use `#[derive(AppError)]` for custom enums.
//!
//! # Examples
//!
//! ```
//! use ladoo::error::Error;
//! use http::StatusCode;
//!
//! // Level 1: any std error becomes 500
//! let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
//! let err: Error = io_err.into();
//! assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
//!
//! // Level 2: explicit status code
//! let err = Error::not_found("user not found");
//! assert_eq!(err.status(), StatusCode::NOT_FOUND);
//! ```

use bytes::Bytes;
use http::StatusCode;

use crate::response::{IntoResponse, Response};

/// A unified HTTP error.
///
/// Stores a status code, a production-safe message, an optional detail
/// string (shown only in dev mode), and an optional error source for
/// chaining.
///
/// `Error` deliberately does **not** implement [`std::error::Error`].
/// This allows the blanket `From<E: std::error::Error>` impl to exist
/// without conflicting with the identity `From<Error> for Error`.
pub struct Error {
    status: StatusCode,
    message: String,
    detail: Option<String>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Create an error with the given status and message.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            detail: None,
            source: None,
        }
    }

    /// Create a 404 Not Found error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// Create a 400 Bad Request error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// Create a 401 Unauthorized error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    /// Create a 403 Forbidden error.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// Create a 500 Internal Server Error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Create a 422 Unprocessable Entity error.
    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    /// Create a 409 Conflict error.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// Attach a dev-only detail string to this error.
    ///
    /// The detail is intended for logging or dev-mode responses, not for
    /// display to end users in production.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attach a source error for chaining and debugging.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The HTTP status code for this error.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The production-safe message for this error.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The dev-only detail string, if any.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Error")
            .field("status", &self.status.as_u16())
            .field("message", &self.message)
            .field("detail", &self.detail)
            .field("source", &self.source)
            .finish()
    }
}

impl<E> From<E> for Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        Error {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Internal Server Error".to_string(),
            detail: Some(err.to_string()),
            source: Some(Box::new(err)),
        }
    }
}

/// A `Result` alias using [`Error`] as the error type.
///
/// This is the standard return type for handlers using the unified error system.
///
/// # Examples
///
/// ```
/// use ladoo::error::{Error, Result};
///
/// fn might_fail() -> Result<String> {
///     Ok("success".to_string())
/// }
/// ```
pub type Result<T> = std::result::Result<T, Error>;

/// Check whether the application is running in development mode.
///
/// Returns `true` unless `LADOO_ENV` or `APP_ENV` is set to
/// `"production"` or `"staging"`. Defaults to `true` (dev mode)
/// when neither variable is set.
///
/// # Examples
///
/// ```
/// use ladoo::error::is_dev_mode;
///
/// std::env::remove_var("LADOO_ENV");
/// std::env::remove_var("APP_ENV");
/// assert!(is_dev_mode());
/// ```
pub fn is_dev_mode() -> bool {
    match std::env::var("LADOO_ENV").or_else(|_| std::env::var("APP_ENV")) {
        Ok(env) => env != "production" && env != "staging",
        Err(_) => true,
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if is_dev_mode() {
            self.render_dev_html()
        } else {
            self.render_prod()
        }
    }
}

impl Error {
    fn render_dev_html(&self) -> Response {
        let status = self.status;
        let reason = status.canonical_reason().unwrap_or("");
        let message = escape_html(&self.message);

        let detail_section = match &self.detail {
            Some(detail) => format!(
                "<div class=\"detail\"><h3>Detail</h3><p>{}</p></div>",
                escape_html(detail)
            ),
            None => String::new(),
        };

        let chain = self.error_chain();
        let chain_section = if chain.is_empty() {
            String::new()
        } else {
            let items: String = chain
                .iter()
                .map(|err| format!("<li>{}</li>", escape_html(err)))
                .collect();
            format!(
                "<div class=\"chain\"><h3>Error Chain</h3><ul>{items}</ul></div>"
            )
        };

        let html = format!(
            r#"<!DOCTYPE html>
<html>
<head>
<title>{status} {reason}</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; max-width: 800px; margin: 40px auto; padding: 0 20px; color: #333; background: #f5f5f5; }}
h1 {{ color: #e74c3c; font-size: 2em; margin-bottom: 0.2em; }}
.message {{ background: #fff; border: 1px solid #ddd; border-radius: 6px; padding: 16px 20px; margin: 16px 0; font-size: 1.1em; }}
.detail {{ background: #fff3cd; border: 1px solid #ffc107; border-radius: 6px; padding: 16px 20px; margin: 16px 0; }}
.detail h3 {{ margin-top: 0; color: #856404; }}
.chain {{ background: #f8f9fa; border: 1px solid #dee2e6; border-radius: 6px; padding: 16px 20px; margin: 16px 0; }}
.chain h3 {{ margin-top: 0; color: #495057; }}
.chain li {{ margin: 6px 0; font-family: "SF Mono", Monaco, monospace; font-size: 0.9em; }}
.footer {{ color: #999; font-size: 0.85em; margin-top: 40px; padding-top: 16px; border-top: 1px solid #ddd; }}
</style>
</head>
<body>
<h1>{status} {reason}</h1>
<div class="message">{message}</div>
{detail_section}
{chain_section}
<div class="footer">Ladoo v0.1 &mdash; Development Error Page</div>
</body>
</html>"#,
            status = status.as_u16(),
            reason = reason,
            message = message,
            detail_section = detail_section,
            chain_section = chain_section,
        );

        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        Response::new(status, headers, Bytes::from(html))
    }

    fn error_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current: Option<&dyn std::error::Error> =
            self.source.as_ref().map(|s| s.as_ref() as &dyn std::error::Error);
        while let Some(err) = current {
            chain.push(err.to_string());
            current = err.source();
        }
        chain
    }

    #[cfg(feature = "json")]
    fn render_prod(&self) -> Response {
        let body = serde_json::json!({
            "error": self.message,
            "status": self.status.as_u16(),
        });
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Response::new(
            self.status,
            headers,
            Bytes::from(body.to_string()),
        )
    }

    #[cfg(not(feature = "json"))]
    fn render_prod(&self) -> Response {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        Response::new(self.status, headers, Bytes::from(self.message.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;

    #[test]
    fn error_new_sets_status_and_message() {
        let err = Error::new(StatusCode::NOT_FOUND, "not found");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        assert_eq!(err.message(), "not found");
        assert!(err.detail().is_none());
    }

    #[test]
    fn error_not_found_sets_404() {
        let err = Error::not_found("user not found");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        assert_eq!(err.message(), "user not found");
    }

    #[test]
    fn error_bad_request_sets_400() {
        let err = Error::bad_request("invalid input");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        assert_eq!(err.message(), "invalid input");
    }

    #[test]
    fn error_unauthorized_sets_401() {
        let err = Error::unauthorized("login required");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(err.message(), "login required");
    }

    #[test]
    fn error_forbidden_sets_403() {
        let err = Error::forbidden("not allowed");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        assert_eq!(err.message(), "not allowed");
    }

    #[test]
    fn error_internal_sets_500() {
        let err = Error::internal("something broke");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.message(), "something broke");
    }

    #[test]
    fn error_unprocessable_sets_422() {
        let err = Error::unprocessable("validation failed");
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(err.message(), "validation failed");
    }

    #[test]
    fn error_conflict_sets_409() {
        let err = Error::conflict("already exists");
        assert_eq!(err.status(), StatusCode::CONFLICT);
        assert_eq!(err.message(), "already exists");
    }

    #[test]
    fn with_detail_adds_detail() {
        let err = Error::not_found("user not found")
            .with_detail("user_id=42 was not in the database");
        assert_eq!(err.detail(), Some("user_id=42 was not in the database"));
    }

    #[test]
    fn with_source_preserves_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = Error::internal("read failed").with_source(io_err);
        assert_eq!(err.message(), "read failed");
        // Source is stored — we verify via Debug output since Error doesn't expose source() publicly
        let debug = format!("{:?}", err);
        assert!(debug.contains("file missing"));
    }

    #[test]
    fn display_shows_message() {
        let err = Error::not_found("user not found");
        assert_eq!(format!("{err}"), "user not found");
    }

    #[test]
    fn debug_shows_status_and_message() {
        let err = Error::bad_request("invalid");
        let debug = format!("{err:?}");
        assert!(debug.contains("400"));
        assert!(debug.contains("invalid"));
    }

    #[test]
    fn from_std_error_creates_500() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: Error = io_err.into();
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.message(), "Internal Server Error");
        assert!(err.detail().unwrap().contains("pipe broke"));
    }

    #[test]
    fn from_std_error_preserves_source_in_detail() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
        let err: Error = io_err.into();
        assert_eq!(err.detail(), Some("disk full"));
    }

    #[test]
    fn result_type_alias_works() {
        fn returns_ok() -> Result<String> {
            Ok("hello".to_string())
        }
        fn returns_err() -> Result<String> {
            Err(Error::not_found("nope"))
        }
        assert!(returns_ok().is_ok());
        assert!(returns_err().is_err());
    }

    #[test]
    fn question_mark_converts_std_error() {
        fn fallible() -> Result<()> {
            let invalid_utf8: Vec<u8> = vec![0xFF];
            let _: String = std::str::from_utf8(&invalid_utf8)
                .map(|s| s.to_string())?;
            Ok(())
        }
        let err = fallible().unwrap_err();
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }

    #[test]
    fn with_detail_string_owned() {
        let err = Error::internal("boom").with_detail(String::from("owned detail"));
        assert_eq!(err.detail(), Some("owned detail"));
    }

    #[test]
    fn new_with_string_message() {
        let msg = String::from("dynamic message");
        let err = Error::new(StatusCode::IM_A_TEAPOT, msg);
        assert_eq!(err.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(err.message(), "dynamic message");
    }

    use crate::response::IntoResponse;
    use std::sync::Mutex;

    // `is_dev_mode` reads process-global environment variables, and `cargo
    // test` runs tests in parallel by default. Serialize every test that
    // touches `LADOO_ENV`/`APP_ENV` through this lock so they don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn error_into_response_dev_mode_returns_html() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        let err = Error::not_found("user not found");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp.content_type(), Some("text/html; charset=utf-8"));
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.contains("404"));
        assert!(body.contains("user not found"));
    }

    #[test]
    fn error_into_response_dev_mode_includes_detail() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        let err = Error::internal("server error")
            .with_detail("connection pool exhausted");
        let resp = err.into_response();
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.contains("connection pool exhausted"));
    }

    #[test]
    fn error_into_response_dev_mode_includes_error_chain() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        let inner = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
        let err = Error::internal("write failed").with_source(inner);
        let resp = err.into_response();
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.contains("disk full"));
    }

    #[test]
    fn error_into_response_prod_mode_returns_json() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        let err = Error::not_found("user not found");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        std::env::remove_var("LADOO_ENV");
        // With json feature, content type is application/json
        #[cfg(feature = "json")]
        {
            assert_eq!(resp.content_type(), Some("application/json"));
            let body = std::str::from_utf8(resp.body_bytes()).unwrap();
            let json: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(json["error"], "user not found");
            assert_eq!(json["status"], 404);
        }
        #[cfg(not(feature = "json"))]
        {
            assert_eq!(resp.content_type(), Some("text/plain; charset=utf-8"));
            assert_eq!(resp.body_bytes(), b"user not found");
        }
    }

    #[test]
    fn error_into_response_prod_mode_hides_detail() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        let err = Error::internal("server error")
            .with_detail("secret database info");
        let resp = err.into_response();
        std::env::remove_var("LADOO_ENV");
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(!body.contains("secret database info"));
    }

    #[test]
    fn error_into_response_staging_is_prod_mode() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "staging");
        let err = Error::not_found("not found");
        let resp = err.into_response();
        std::env::remove_var("LADOO_ENV");
        // Staging uses prod rendering (not HTML)
        assert_ne!(resp.content_type(), Some("text/html; charset=utf-8"));
    }

    #[test]
    fn app_env_fallback() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::set_var("APP_ENV", "production");
        let err = Error::bad_request("bad");
        let resp = err.into_response();
        std::env::remove_var("APP_ENV");
        assert_ne!(resp.content_type(), Some("text/html; charset=utf-8"));
    }

    #[test]
    fn is_dev_mode_default_true() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        assert!(is_dev_mode());
    }

    #[test]
    fn is_dev_mode_production_false() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        assert!(!is_dev_mode());
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn is_dev_mode_development_true() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "development");
        assert!(is_dev_mode());
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn result_ok_into_response() {
        let result: std::result::Result<&str, Error> = Ok("hello");
        let resp = result.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"hello");
    }

    #[test]
    fn result_err_into_response() {
        let _guard = lock_env();
        std::env::set_var("LADOO_ENV", "production");
        let result: std::result::Result<&str, Error> = Err(Error::not_found("gone"));
        let resp = result.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        std::env::remove_var("LADOO_ENV");
    }

    #[test]
    fn result_with_custom_error_type() {
        struct MyError;
        impl IntoResponse for MyError {
            fn into_response(self) -> crate::response::Response {
                (StatusCode::IM_A_TEAPOT, "teapot").into_response()
            }
        }
        let result: std::result::Result<&str, MyError> = Err(MyError);
        let resp = result.into_response();
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    }

    #[test]
    fn dev_html_page_has_ladoo_footer() {
        let _guard = lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        let err = Error::internal("boom");
        let resp = err.into_response();
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.contains("Ladoo"));
    }
}
