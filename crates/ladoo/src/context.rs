//! Middleware context.
//!
//! [`Context`] wraps a [`Request`] and is the primary type middleware
//! functions interact with. It provides the same accessors as `Request`
//! (method, path, headers, params, body) while letting the framework
//! control when the inner request is consumed.
//!
//! # Examples
//!
//! ```
//! use ladoo::context::Context;
//! use ladoo::request::Request;
//! use http::Method;
//!
//! let req = Request::test(Method::GET, "/users/42");
//! let ctx = Context::new(req);
//! assert_eq!(ctx.method(), Method::GET);
//! assert_eq!(ctx.path(), "/users/42");
//! ```

use http::{HeaderMap, Method, Uri};

use crate::request::Request;

/// The middleware context.
///
/// Wraps a [`Request`] and is passed through the middleware chain.
/// Middleware receives an owned `Context`, inspects or modifies the
/// request through its accessors, then passes it to
/// [`Next::run`](crate::middleware::Next::run) to continue the chain.
///
/// When the innermost middleware (or the handler itself) needs the raw
/// [`Request`], call [`into_request`](Context::into_request).
pub struct Context {
    request: Request,
}

impl Context {
    /// Create a new context wrapping the given request.
    pub fn new(request: Request) -> Self {
        Self { request }
    }

    /// Returns the HTTP method.
    pub fn method(&self) -> &Method {
        self.request.method()
    }

    /// Returns the request path without the query string.
    pub fn path(&self) -> &str {
        self.request.path()
    }

    /// Returns the full URI including query string.
    pub fn uri(&self) -> &Uri {
        self.request.uri()
    }

    /// Returns the request headers.
    pub fn headers(&self) -> &HeaderMap {
        self.request.headers()
    }

    /// Returns a path parameter by name.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.request.param(name)
    }

    /// Returns the request body as bytes.
    pub fn body(&self) -> &[u8] {
        self.request.body()
    }

    /// Consume the context and return the inner request.
    pub fn into_request(self) -> Request {
        self.request
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Request;
    use http::Method;

    #[test]
    fn method_delegates_to_request() {
        let req = Request::test(Method::POST, "/users");
        let ctx = Context::new(req);
        assert_eq!(ctx.method(), Method::POST);
    }

    #[test]
    fn path_delegates_to_request() {
        let req = Request::test(Method::GET, "/users/42");
        let ctx = Context::new(req);
        assert_eq!(ctx.path(), "/users/42");
    }

    #[test]
    fn uri_delegates_to_request() {
        let req = Request::test(Method::GET, "/search?q=rust");
        let ctx = Context::new(req);
        assert_eq!(ctx.uri().path(), "/search");
    }

    #[test]
    fn headers_delegates_to_request() {
        let req = Request::test(Method::GET, "/");
        let ctx = Context::new(req);
        assert!(ctx.headers().is_empty());
    }

    #[test]
    fn param_delegates_to_request() {
        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(vec![("id".into(), "42".into())]);
        let ctx = Context::new(req);
        assert_eq!(ctx.param("id"), Some("42"));
    }

    #[test]
    fn param_returns_none_for_missing() {
        let req = Request::test(Method::GET, "/");
        let ctx = Context::new(req);
        assert_eq!(ctx.param("id"), None);
    }

    #[test]
    fn body_delegates_to_request() {
        let req = Request::test_with_body(Method::POST, "/", b"hello");
        let ctx = Context::new(req);
        assert_eq!(ctx.body(), b"hello");
    }

    #[test]
    fn into_request_returns_inner() {
        let req = Request::test(Method::GET, "/test");
        let ctx = Context::new(req);
        let req = ctx.into_request();
        assert_eq!(req.path(), "/test");
    }
}
