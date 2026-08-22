//! HTTP request type.
//!
//! [`Request`] provides access to the HTTP method, path, headers,
//! and route parameters extracted during path matching.
//!
//! # Examples
//!
//! ```
//! use ladoo::request::Request;
//! use http::Method;
//!
//! let req = Request::test(Method::GET, "/users/42");
//! assert_eq!(req.method(), Method::GET);
//! assert_eq!(req.path(), "/users/42");
//! ```

use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, Method, Uri};

use crate::state::TypeMap;

/// Parameters extracted from the URL path during route matching.
///
/// Stored as name-value pairs. For example, matching the pattern
/// `/users/:id` against `/users/42` produces `[("id", "42")]`.
pub type PathParams = Vec<(String, String)>;

/// An HTTP request received by a handler.
///
/// Provides access to the request method, path, headers, body, and any
/// path parameters extracted during route matching.
pub struct Request {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    params: PathParams,
    body: Bytes,
    extensions: Arc<TypeMap>,
    per_request: TypeMap,
    peer_ip: Option<String>,
}

impl Clone for Request {
    fn clone(&self) -> Self {
        Self {
            method: self.method.clone(),
            uri: self.uri.clone(),
            headers: self.headers.clone(),
            params: self.params.clone(),
            body: self.body.clone(),
            extensions: self.extensions.clone(),
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }
}

impl Request {
    /// Create a new request from its parts.
    ///
    /// Called internally when converting a hyper request.
    pub(crate) fn new(
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        params: PathParams,
        body: Bytes,
        extensions: Arc<TypeMap>,
    ) -> Self {
        Self {
            method,
            uri,
            headers,
            params,
            body,
            extensions,
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }

    /// Create a test request with the given method and path.
    ///
    /// Useful for unit testing handlers and extractors.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use http::Method;
    ///
    /// let req = Request::test(Method::GET, "/users/42");
    /// assert_eq!(req.path(), "/users/42");
    /// ```
    pub fn test(method: Method, path: &str) -> Self {
        Self {
            method,
            uri: path.parse().expect("invalid URI"),
            headers: HeaderMap::new(),
            params: Vec::new(),
            body: Bytes::new(),
            extensions: Arc::new(TypeMap::new()),
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }

    /// Create a test request with a body.
    ///
    /// Useful for unit testing handlers and extractors that read the
    /// request body, such as `Json<T>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use http::Method;
    ///
    /// let req = Request::test_with_body(Method::POST, "/", b"hello");
    /// assert_eq!(req.body(), b"hello");
    /// ```
    pub fn test_with_body(method: Method, path: &str, body: &[u8]) -> Self {
        Self {
            method,
            uri: path.parse().expect("invalid URI"),
            headers: HeaderMap::new(),
            params: Vec::new(),
            body: Bytes::copy_from_slice(body),
            extensions: Arc::new(TypeMap::new()),
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }

    /// Create a test request with custom headers.
    ///
    /// Useful for unit testing handlers and extractors that read
    /// headers, such as `Content-Type` detection.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use http::Method;
    ///
    /// let mut headers = http::HeaderMap::new();
    /// headers.insert(
    ///     http::header::CONTENT_TYPE,
    ///     http::HeaderValue::from_static("application/json"),
    /// );
    /// let req = Request::test_with_headers(Method::POST, "/", headers);
    /// assert_eq!(req.content_type(), Some("application/json"));
    /// ```
    pub fn test_with_headers(method: Method, path: &str, headers: HeaderMap) -> Self {
        Self {
            method,
            uri: path.parse().expect("invalid URI"),
            headers,
            params: Vec::new(),
            body: Bytes::new(),
            extensions: Arc::new(TypeMap::new()),
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }

    /// Create a test request with a JSON body and `Content-Type: application/json`.
    ///
    /// Useful for unit testing extractors that both require the JSON
    /// content type and read the body, such as `Json<T>` and `Valid<Json<T>>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use http::Method;
    ///
    /// let req = Request::test_with_json(Method::POST, "/users", br#"{"name":"Alice"}"#);
    /// assert_eq!(req.content_type(), Some("application/json"));
    /// assert_eq!(req.body(), br#"{"name":"Alice"}"#);
    /// ```
    pub fn test_with_json(method: Method, path: &str, body: &[u8]) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Self {
            method,
            uri: path.parse().expect("invalid URI"),
            headers,
            params: Vec::new(),
            body: Bytes::copy_from_slice(body),
            extensions: Arc::new(TypeMap::new()),
            per_request: TypeMap::new(),
            peer_ip: None,
        }
    }

    /// Returns the request body as bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Takes the request body, leaving an empty body in its place.
    ///
    /// Body-consuming extractors (like `Json<T>`) call this so the body
    /// is only parsed once.
    pub fn take_body(&mut self) -> Bytes {
        std::mem::take(&mut self.body)
    }

    /// Returns the `Content-Type` header value, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
    }

    /// Returns the HTTP method (GET, POST, etc.).
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// Returns the request path without the query string.
    ///
    /// For `/search?q=rust`, returns `/search`.
    pub fn path(&self) -> &str {
        self.uri.path()
    }

    /// Returns the full URI including query string.
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Returns the request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns the value of a path parameter extracted during route matching.
    ///
    /// Returns `None` if the parameter name was not matched.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use http::Method;
    ///
    /// let req = Request::test(Method::GET, "/users/42");
    /// // In a real handler, the router would set params automatically
    /// assert_eq!(req.param("id"), None);  // no params set yet
    /// ```
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Returns all path parameters as a slice.
    pub fn params(&self) -> &[(String, String)] {
        &self.params
    }

    /// Set the path parameters. Used by the router after matching.
    #[allow(dead_code)]
    pub(crate) fn set_params(&mut self, params: PathParams) {
        self.params = params;
    }

    /// Returns the application state map registered via `App::provide`.
    pub(crate) fn extensions(&self) -> &TypeMap {
        &self.extensions
    }

    /// Returns the application state map as an owned, cheaply-cloned
    /// `Arc`, for handlers that need to hand it to a spawned task rather
    /// than borrowing it for the lifetime of the request (e.g. the
    /// WebSocket channel connection loop, which outlives the request that
    /// upgraded it).
    #[cfg(feature = "ws")]
    pub(crate) fn extensions_arc(&self) -> Arc<TypeMap> {
        Arc::clone(&self.extensions)
    }

    /// Insert a value into this request's per-request state.
    ///
    /// Used by framework middleware to inject request-scoped values
    /// (like `RequestId`) that handlers can extract via `State<T>`.
    pub(crate) fn provide<T: Send + Sync + 'static>(&mut self, value: T) {
        self.per_request.insert_shared(value);
    }

    /// Returns the per-request state map.
    pub(crate) fn per_request(&self) -> &TypeMap {
        &self.per_request
    }

    /// Register a value on this request's application state, for testing.
    ///
    /// Lets you unit test handlers and extractors that depend on
    /// [`State<T>`](crate::state::State) without starting a server or
    /// building a full [`App`](crate::app::App).
    ///
    /// # Panics
    ///
    /// Panics if the request's state is shared with another `Request`
    /// (for example, after cloning). Test requests own their state
    /// exclusively, so this should never happen in practice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ladoo::request::Request;
    /// use ladoo::state::State;
    /// use ladoo::extract::FromRequest;
    /// use http::Method;
    ///
    /// let mut req = Request::test(Method::GET, "/");
    /// req.provide_test_state(42_u32);
    /// let state = State::<u32>::from_request(&mut req).unwrap();
    /// assert_eq!(*state, 42);
    /// ```
    pub fn provide_test_state<T: Send + Sync + 'static>(&mut self, value: T) {
        Arc::get_mut(&mut self.extensions)
            .expect("provide_test_state: request state is shared, cannot mutate")
            .insert_shared(value);
    }

    /// Returns the client's IP address, if known.
    ///
    /// This is set from the TCP socket peer address when running under
    /// a real server. In-memory test requests return `None` unless
    /// [`set_peer_ip`](Request::set_peer_ip) is called.
    pub fn peer_ip(&self) -> Option<&str> {
        self.peer_ip.as_deref()
    }

    /// Set the client's IP address.
    ///
    /// Called internally by the server after accepting a TCP connection.
    /// Also available in tests to simulate different client IPs.
    pub fn set_peer_ip(&mut self, ip: String) {
        self.peer_ip = Some(ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FromRequest;

    #[test]
    fn method_returns_http_method() {
        let req = Request::test(Method::POST, "/users");
        assert_eq!(req.method(), Method::POST);
    }

    #[test]
    fn path_returns_uri_path() {
        let req = Request::test(Method::GET, "/users/42");
        assert_eq!(req.path(), "/users/42");
    }

    #[test]
    fn param_returns_matched_value() {
        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(vec![("id".into(), "42".into())]);
        assert_eq!(req.param("id"), Some("42"));
    }

    #[test]
    fn param_returns_none_for_missing_key() {
        let req = Request::test(Method::GET, "/users/42");
        assert_eq!(req.param("id"), None);
    }

    #[test]
    fn headers_returns_header_map() {
        let req = Request::test(Method::GET, "/");
        assert!(req.headers().is_empty());
    }

    #[test]
    fn path_with_query_string_returns_only_path() {
        let req = Request::test(Method::GET, "/search?q=rust");
        assert_eq!(req.path(), "/search");
    }

    #[test]
    fn multiple_params_accessible() {
        let mut req = Request::test(Method::GET, "/users/42/posts/7");
        req.set_params(vec![
            ("user_id".into(), "42".into()),
            ("post_id".into(), "7".into()),
        ]);
        assert_eq!(req.param("user_id"), Some("42"));
        assert_eq!(req.param("post_id"), Some("7"));
    }

    #[test]
    fn root_path() {
        let req = Request::test(Method::GET, "/");
        assert_eq!(req.path(), "/");
    }

    #[test]
    fn body_returns_request_body() {
        let req = Request::test_with_body(Method::POST, "/", b"hello body");
        assert_eq!(req.body(), b"hello body");
    }

    #[test]
    fn take_body_returns_and_empties_body() {
        let mut req = Request::test_with_body(Method::POST, "/", b"take me");
        let body = req.take_body();
        assert_eq!(body.as_ref(), b"take me");
        assert!(req.body().is_empty());
    }

    #[test]
    fn content_type_returns_header_value() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        let req = Request::test_with_headers(Method::POST, "/", headers);
        assert_eq!(req.content_type(), Some("application/json"));
    }

    #[test]
    fn content_type_returns_none_when_missing() {
        let req = Request::test(Method::GET, "/");
        assert_eq!(req.content_type(), None);
    }

    #[test]
    fn test_request_has_empty_body() {
        let req = Request::test(Method::GET, "/");
        assert!(req.body().is_empty());
    }

    #[test]
    fn clone_request_preserves_all_fields() {
        let mut req = Request::test_with_body(Method::POST, "/users", b"body data");
        req.set_params(vec![("id".into(), "42".into())]);
        let cloned = req.clone();
        assert_eq!(cloned.method(), Method::POST);
        assert_eq!(cloned.path(), "/users");
        assert_eq!(cloned.body(), b"body data");
        assert_eq!(cloned.param("id"), Some("42"));
    }

    #[test]
    fn test_request_has_empty_extensions() {
        let req = Request::test(Method::GET, "/");
        assert!(!req.extensions().contains_shared::<u32>());
    }

    #[test]
    fn provide_test_state_makes_value_visible_in_extensions() {
        let mut req = Request::test(Method::GET, "/");
        req.provide_test_state(42_u32);
        assert_eq!(*req.extensions().get_shared::<u32>().unwrap(), 42);
    }

    #[test]
    fn provide_test_state_supports_multiple_types() {
        let mut req = Request::test(Method::GET, "/");
        req.provide_test_state(42_u32);
        req.provide_test_state(String::from("hello"));
        assert_eq!(*req.extensions().get_shared::<u32>().unwrap(), 42);
        assert_eq!(
            *req.extensions().get_shared::<String>().unwrap(),
            String::from("hello")
        );
    }

    #[test]
    fn clone_resets_per_request_state() {
        let mut req = Request::test(Method::GET, "/");
        req.provide(42_u32);
        let mut cloned = req.clone();
        assert!(crate::state::State::<u32>::from_request(&mut cloned).is_err());
    }

    #[test]
    fn peer_ip_returns_stored_ip() {
        let mut req = Request::test(Method::GET, "/");
        req.set_peer_ip("192.168.1.1".to_string());
        assert_eq!(req.peer_ip(), Some("192.168.1.1"));
    }

    #[test]
    fn peer_ip_returns_none_when_not_set() {
        let req = Request::test(Method::GET, "/");
        assert_eq!(req.peer_ip(), None);
    }
}
