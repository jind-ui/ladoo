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

    /// Borrow the inner request.
    ///
    /// Used by auth middleware to pass the request to
    /// [`AuthProvider::authenticate`](crate::auth::AuthProvider::authenticate)
    /// without consuming the context. Not yet called outside tests — a
    /// future task wires `AuthGuardMiddleware` into `Router`.
    #[allow(dead_code)]
    pub(crate) fn request(&self) -> &Request {
        &self.request
    }

    /// Insert a value into per-request state.
    ///
    /// Values inserted here are available to downstream middleware and
    /// handlers via [`State<T>`](crate::state::State) extraction.
    /// Per-request state takes precedence over app-level state from
    /// [`App::provide`](crate::app::App::provide).
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// async fn my_middleware(mut ctx: Context, next: Next) -> Result<Response> {
    ///     ctx.provide(RequestId("custom-id".into()));
    ///     next.run(ctx).await
    /// }
    /// ```
    pub fn provide<T: Send + Sync + 'static>(&mut self, value: T) {
        self.request.provide(value);
    }

    /// Read a value from per-request state.
    ///
    /// Only called by the `logging` feature's request logger today; when
    /// that feature is disabled this method has no non-test callers.
    #[cfg_attr(not(feature = "logging"), allow(dead_code))]
    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.request.per_request().get::<T>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FromRequest;
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

    #[test]
    fn request_borrows_inner_without_consuming_context() {
        let req = Request::test(Method::GET, "/test");
        let ctx = Context::new(req);
        assert_eq!(ctx.request().path(), "/test");
        // ctx is still usable after borrowing the inner request.
        assert_eq!(ctx.path(), "/test");
    }

    #[test]
    fn provide_inserts_per_request_state() {
        let req = Request::test(Method::GET, "/");
        let mut ctx = Context::new(req);
        ctx.provide(42_u32);
        let req = ctx.into_request();
        let mut req = req;
        let extracted = crate::state::State::<u32>::from_request(&mut req).unwrap();
        assert_eq!(*extracted, 42);
    }

    #[test]
    fn get_reads_per_request_state() {
        let req = Request::test(Method::GET, "/");
        let mut ctx = Context::new(req);
        ctx.provide(42_u32);
        assert_eq!(ctx.get::<u32>(), Some(&42));
    }

    #[test]
    fn get_returns_none_for_missing() {
        let req = Request::test(Method::GET, "/");
        let ctx = Context::new(req);
        assert_eq!(ctx.get::<u32>(), None);
    }
}
