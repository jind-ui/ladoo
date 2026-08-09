//! HTTP response type and the [`IntoResponse`] trait.
//!
//! Any type implementing [`IntoResponse`] can be returned from a handler.
//! The framework provides implementations for common types like strings,
//! status codes, and tuples.
//!
//! # Examples
//!
//! ```
//! use ladoo::response::IntoResponse;
//!
//! // Strings become 200 OK with text body
//! let resp = "Hello".to_string().into_response();
//! assert_eq!(resp.status(), 200);
//! ```

use bytes::Bytes;
use http::StatusCode;
use http_body_util::Full;

/// An HTTP response.
///
/// Stores the status, headers, and body separately, and is reconstructed
/// into a [`hyper::Response`] internally when sent to the client.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Response {
    /// Create a new response from a status, headers, and body.
    pub(crate) fn new(status: StatusCode, headers: http::HeaderMap, body: Bytes) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns the HTTP status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns the response body as bytes.
    ///
    /// Useful for testing and debugging.
    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }

    /// Returns the Content-Type header value, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
    }

    /// Return a new response with the given status code.
    pub(crate) fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Consume this response and return the equivalent hyper response.
    ///
    /// Used internally when sending the response to the client.
    pub(crate) fn into_hyper(self) -> hyper::Response<Full<Bytes>> {
        let mut builder = hyper::Response::builder().status(self.status);
        if let Some(headers) = builder.headers_mut() {
            *headers = self.headers;
        }
        builder
            .body(Full::new(self.body))
            .expect("status and headers are already validated")
    }
}

/// Convert a type into an HTTP [`Response`].
///
/// Implement this trait to return custom types from handlers.
/// The framework provides implementations for common types.
///
/// # Examples
///
/// ```
/// use ladoo::response::{IntoResponse, Response};
///
/// // A simple string response
/// let response = "Hello World".into_response();
/// assert_eq!(response.status(), 200);
/// ```
pub trait IntoResponse {
    /// Convert this type into an HTTP response.
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        Response::new(StatusCode::OK, headers, Bytes::from(self))
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        self.to_string().into_response()
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        Response::new(self, http::HeaderMap::new(), Bytes::new())
    }
}

/// Overrides the status code of any [`IntoResponse`] value.
///
/// # Examples
///
/// ```
/// use ladoo::response::IntoResponse;
/// use http::StatusCode;
///
/// let resp = (StatusCode::CREATED, "created").into_response();
/// assert_eq!(resp.status(), StatusCode::CREATED);
/// ```
impl<T: IntoResponse> IntoResponse for (StatusCode, T) {
    fn into_response(self) -> Response {
        self.1.into_response().with_status(self.0)
    }
}

/// An HTML response.
///
/// Wraps a string and sets `Content-Type: text/html; charset=utf-8`.
///
/// # Examples
///
/// ```
/// use ladoo::response::{Html, IntoResponse};
///
/// let resp = Html("<h1>Hello</h1>".to_string()).into_response();
/// assert_eq!(resp.content_type(), Some("text/html; charset=utf-8"));
/// ```
pub struct Html(pub String);

impl IntoResponse for Html {
    fn into_response(self) -> Response {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        Response::new(StatusCode::OK, headers, Bytes::from(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_into_response_returns_200_with_body() {
        let resp = "Hello World".to_string().into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"Hello World");
        assert_eq!(resp.content_type(), Some("text/plain; charset=utf-8"));
    }

    #[test]
    fn static_str_into_response_returns_200_with_body() {
        let resp = "Hello".into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"Hello");
    }

    #[test]
    fn status_code_into_response_returns_status_with_empty_body() {
        let resp = StatusCode::NOT_FOUND.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp.body_bytes(), b"");
    }

    #[test]
    fn tuple_status_str_into_response_returns_status_with_body() {
        let resp = (StatusCode::CREATED, "created").into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(resp.body_bytes(), b"created");
    }

    #[test]
    fn tuple_status_string_into_response_returns_status_with_body() {
        let resp = (StatusCode::BAD_REQUEST, "bad".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.body_bytes(), b"bad");
    }

    #[test]
    fn response_into_response_is_identity() {
        let original = "test".into_response();
        let status = original.status();
        let resp = original.into_response();
        assert_eq!(resp.status(), status);
    }

    #[test]
    fn empty_string_into_response() {
        let resp = "".into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"");
    }

    #[test]
    fn html_into_response_sets_content_type() {
        let resp = Html("<h1>Hello</h1>".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.content_type(), Some("text/html; charset=utf-8"));
        assert_eq!(resp.body_bytes(), b"<h1>Hello</h1>");
    }

    #[test]
    fn html_from_str_into_response() {
        let resp = Html("<p>world</p>".to_string()).into_response();
        assert_eq!(resp.body_bytes(), b"<p>world</p>");
    }

    #[test]
    fn generic_tuple_with_html() {
        let resp = (StatusCode::OK, Html("<h1>Hi</h1>".to_string())).into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.content_type(), Some("text/html; charset=utf-8"));
    }
}
