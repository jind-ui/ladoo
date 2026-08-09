//! Handler trait and conversion utilities.
//!
//! Handlers process HTTP requests and produce responses. They are stored
//! internally as `Box<dyn Handler>` for fast compilation — one compiled
//! version regardless of how many handlers an app has.
//!
//! Both sync and async closures are supported via [`IntoHandler`]:
//!
//! ```
//! use ladoo::handler::IntoHandler;
//! use ladoo::request::Request;
//!
//! // Sync handler — returns a value directly
//! let _h = (|_req: Request| "Hello World").into_handler();
//!
//! // Async handler — returns a future
//! let _h = (|_req: Request| async { "Hello World" }).into_handler();
//! ```

use std::future::Future;
use std::pin::Pin;

use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// An object-safe HTTP request handler.
///
/// All handlers are stored as `Box<dyn Handler>` — this means one compiled
/// version of the routing code regardless of how many handlers exist.
/// The ~2ns vtable lookup cost is negligible compared to actual handler work.
///
/// You rarely implement this trait directly. Instead, use closures with
/// [`IntoHandler`], which auto-wraps both sync and async functions.
pub trait Handler: Send + std::marker::Sync {
    /// Process an HTTP request and return a response.
    fn call(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + '_>>;
}

/// Convert a function or closure into a boxed [`Handler`].
///
/// The type parameter `M` is a marker that lets the compiler distinguish
/// between sync handlers (which return a value directly) and async handlers
/// (which return a future). You don't need to specify it — inference handles it.
///
/// # Examples
///
/// ```
/// use ladoo::handler::IntoHandler;
/// use ladoo::request::Request;
///
/// // Sync
/// let handler = (|_req: Request| "Hello").into_handler();
///
/// // Async
/// let handler = (|_req: Request| async { "Hello" }).into_handler();
/// ```
pub trait IntoHandler<M> {
    /// Convert this into a boxed handler.
    fn into_handler(self) -> Box<dyn Handler>;
}

// -- Marker types for distinguishing sync vs async handlers --

/// Marker type for sync handlers (used by [`IntoHandler`] impl).
pub struct Sync;

/// Marker type for async handlers (used by [`IntoHandler`] impl).
pub struct Async;

// -- Wrapper structs that implement Handler --

struct SyncHandlerFn<F> {
    f: F,
}

struct AsyncHandlerFn<F> {
    f: F,
}

// -- IntoHandler for sync closures: Fn(Request) -> R --

impl<F, R> IntoHandler<Sync> for F
where
    F: Fn(Request) -> R + Send + std::marker::Sync + 'static,
    R: IntoResponse + 'static,
{
    fn into_handler(self) -> Box<dyn Handler> {
        Box::new(SyncHandlerFn { f: self })
    }
}

impl<F, R> Handler for SyncHandlerFn<F>
where
    F: Fn(Request) -> R + Send + std::marker::Sync + 'static,
    R: IntoResponse + 'static,
{
    fn call(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
        let response = (self.f)(req).into_response();
        Box::pin(async move { response })
    }
}

// -- IntoHandler for async closures: Fn(Request) -> Future<Output = R> --

impl<F, Fut, R> IntoHandler<Async> for F
where
    F: Fn(Request) -> Fut + Send + std::marker::Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse,
{
    fn into_handler(self) -> Box<dyn Handler> {
        Box::new(AsyncHandlerFn { f: self })
    }
}

impl<F, Fut, R> Handler for AsyncHandlerFn<F>
where
    F: Fn(Request) -> Fut + Send + std::marker::Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse,
{
    fn call(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
        let fut = (self.f)(req);
        Box::pin(async move { fut.await.into_response() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, StatusCode};

    #[tokio::test]
    async fn sync_handler_returns_string_response() {
        let handler = (|_req: Request| "Hello World").into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"Hello World");
    }

    #[tokio::test]
    async fn async_handler_returns_string_response() {
        let handler = (|_req: Request| async { "Hello Async" }).into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"Hello Async");
    }

    #[tokio::test]
    async fn sync_handler_returns_status_code() {
        let handler = (|_req: Request| StatusCode::NOT_FOUND).into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn sync_handler_returns_tuple() {
        let handler =
            (|_req: Request| (StatusCode::CREATED, "created")).into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(resp.body_bytes(), b"created");
    }

    #[tokio::test]
    async fn handler_can_read_request_params() {
        let handler = (|req: Request| {
            let name = req.param("name").unwrap_or("stranger");
            format!("Hello {name}")
        })
        .into_handler();

        let mut req = Request::test(Method::GET, "/greet/Neel");
        req.set_params(vec![("name".into(), "Neel".into())]);

        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"Hello Neel");
    }

    #[tokio::test]
    async fn async_handler_can_read_request_params() {
        let handler = (|req: Request| async move {
            let id = req.param("id").unwrap_or("0");
            format!("User {id}")
        })
        .into_handler();

        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(vec![("id".into(), "42".into())]);

        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"User 42");
    }

    #[tokio::test]
    async fn boxed_handler_is_object_safe() {
        // Verify Box<dyn Handler> works — this is a core design decision
        let handlers: Vec<Box<dyn Handler>> = vec![
            (|_req: Request| "first").into_handler(),
            (|_req: Request| "second").into_handler(),
        ];

        let req = Request::test(Method::GET, "/");
        let resp = handlers[0].call(req).await;
        assert_eq!(resp.body_bytes(), b"first");

        let req = Request::test(Method::GET, "/");
        let resp = handlers[1].call(req).await;
        assert_eq!(resp.body_bytes(), b"second");
    }
}
