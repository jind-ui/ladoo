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

use http::StatusCode;

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
}
