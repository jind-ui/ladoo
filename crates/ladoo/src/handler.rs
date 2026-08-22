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

use crate::extract::FromRequest;
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

/// Marker type for handlers that are already boxed (used by [`IntoHandler`] impl).
///
/// Lets helper functions that build a [`Handler`] directly — like
/// [`websocket()`](crate::ws::websocket) — return `Box<dyn Handler>` and
/// still be passed straight to route-registration methods (`App::get`,
/// etc.) that expect an `IntoHandler` implementor.
pub struct Boxed;

impl IntoHandler<Boxed> for Box<dyn Handler> {
    fn into_handler(self) -> Box<dyn Handler> {
        self
    }
}

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

// -- IntoHandler for extractor-based closures: Fn(T1, T2, ...) -> R --
//
// `Request` deliberately does not implement `FromRequest`, so these impls
// never collide with the `Fn(Request) -> R` impls above: a closure taking a
// bare `Request` argument only matches `IntoHandler<Sync>`/`IntoHandler<Async>`,
// and a closure taking `FromRequest` types only matches the impls generated
// here.

/// Marker type for sync extractor-based handlers (used by [`IntoHandler`] impl).
///
/// Uses `PhantomData<fn() -> T>` rather than `PhantomData<T>` so that this
/// marker (and the wrapper structs below) stay `Send + Sync` regardless of
/// whether the extractor types `T` are — the tuple of extractors is never
/// actually stored, only used to select the right trait impl.
pub struct SyncExtract<T>(std::marker::PhantomData<fn() -> T>);

/// Marker type for async extractor-based handlers (used by [`IntoHandler`] impl).
pub struct AsyncExtract<T>(std::marker::PhantomData<fn() -> T>);

struct SyncExtractHandlerFn<F, T> {
    f: F,
    _marker: std::marker::PhantomData<fn() -> T>,
}

struct AsyncExtractHandlerFn<F, T> {
    f: F,
    _marker: std::marker::PhantomData<fn() -> T>,
}

macro_rules! impl_extract_handler {
    ($(($T:ident, $t:ident)),*) => {
        // Sync extractor handler
        impl<F, $($T,)* R> IntoHandler<SyncExtract<($($T,)*)>> for F
        where
            F: Fn($($T),*) -> R + Send + std::marker::Sync + 'static,
            $($T: FromRequest + 'static,)*
            R: IntoResponse + 'static,
        {
            fn into_handler(self) -> Box<dyn Handler> {
                Box::new(SyncExtractHandlerFn::<F, ($($T,)*)> {
                    f: self,
                    _marker: std::marker::PhantomData,
                })
            }
        }

        impl<F, $($T,)* R> Handler for SyncExtractHandlerFn<F, ($($T,)*)>
        where
            F: Fn($($T),*) -> R + Send + std::marker::Sync + 'static,
            $($T: FromRequest + 'static,)*
            R: IntoResponse + 'static,
        {
            fn call(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
                Box::pin(async move {
                    #[allow(unused_mut)]
                    let mut _req = _req;
                    $(
                        let $t = match $T::from_request(&mut _req) {
                            Ok(v) => v,
                            Err(resp) => return resp,
                        };
                    )*
                    (self.f)($($t),*).into_response()
                })
            }
        }

        // Async extractor handler
        impl<F, $($T,)* Fut, R> IntoHandler<AsyncExtract<($($T,)*)>> for F
        where
            F: Fn($($T),*) -> Fut + Send + std::marker::Sync + 'static,
            Fut: Future<Output = R> + Send + 'static,
            $($T: FromRequest + 'static,)*
            R: IntoResponse + 'static,
        {
            fn into_handler(self) -> Box<dyn Handler> {
                Box::new(AsyncExtractHandlerFn::<F, ($($T,)*)> {
                    f: self,
                    _marker: std::marker::PhantomData,
                })
            }
        }

        impl<F, $($T,)* Fut, R> Handler for AsyncExtractHandlerFn<F, ($($T,)*)>
        where
            F: Fn($($T),*) -> Fut + Send + std::marker::Sync + 'static,
            Fut: Future<Output = R> + Send + 'static,
            $($T: FromRequest + 'static,)*
            R: IntoResponse + 'static,
        {
            fn call(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
                Box::pin(async move {
                    #[allow(unused_mut)]
                    let mut _req = _req;
                    $(
                        let $t = match $T::from_request(&mut _req) {
                            Ok(v) => v,
                            Err(resp) => return resp,
                        };
                    )*
                    (self.f)($($t),*).await.into_response()
                })
            }
        }
    };
}

impl_extract_handler!();
impl_extract_handler!((T1, t1));
impl_extract_handler!((T1, t1), (T2, t2));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3), (T4, t4));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3), (T4, t4), (T5, t5));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3), (T4, t4), (T5, t5), (T6, t6));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3), (T4, t4), (T5, t5), (T6, t6), (T7, t7));
impl_extract_handler!((T1, t1), (T2, t2), (T3, t3), (T4, t4), (T5, t5), (T6, t6), (T7, t7), (T8, t8));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FromRequest;
    use crate::response::IntoResponse;
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

    struct MethodStr(String);
    impl FromRequest for MethodStr {
        fn from_request(req: &mut Request) -> Result<Self, Response> {
            Ok(MethodStr(req.method().to_string()))
        }
    }

    struct PathStr(String);
    impl FromRequest for PathStr {
        fn from_request(req: &mut Request) -> Result<Self, Response> {
            Ok(PathStr(req.path().to_string()))
        }
    }

    #[tokio::test]
    async fn zero_arg_sync_handler() {
        let handler = (|| "no args").into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"no args");
    }

    #[tokio::test]
    async fn zero_arg_async_handler() {
        let handler = (|| async { "async no args" }).into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"async no args");
    }

    #[tokio::test]
    async fn one_extractor_sync_handler() {
        let handler = (|path: PathStr| format!("path: {}", path.0)).into_handler();
        let req = Request::test(Method::GET, "/hello");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"path: /hello");
    }

    #[tokio::test]
    async fn one_extractor_async_handler() {
        let handler =
            (|path: PathStr| async move { format!("async path: {}", path.0) }).into_handler();
        let req = Request::test(Method::GET, "/hello");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"async path: /hello");
    }

    #[tokio::test]
    async fn two_extractor_sync_handler() {
        let handler = (|method: MethodStr, path: PathStr| {
            format!("{} {}", method.0, path.0)
        })
        .into_handler();
        let req = Request::test(Method::POST, "/submit");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"POST /submit");
    }

    #[tokio::test]
    async fn extractor_failure_returns_error_response() {
        struct AlwaysFails;
        impl FromRequest for AlwaysFails {
            fn from_request(_req: &mut Request) -> Result<Self, Response> {
                Err((StatusCode::BAD_REQUEST, "extraction failed").into_response())
            }
        }

        let handler = (|_: AlwaysFails| "unreachable").into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.body_bytes(), b"extraction failed");
    }

    #[tokio::test]
    async fn existing_request_handler_still_works() {
        let handler = (|req: Request| format!("path: {}", req.path())).into_handler();
        let req = Request::test(Method::GET, "/test");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"path: /test");
    }

    #[tokio::test]
    async fn underscore_handler_still_works() {
        let handler = (|_: Request| "still works").into_handler();
        let req = Request::test(Method::GET, "/");
        let resp = handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"still works");
    }
}
