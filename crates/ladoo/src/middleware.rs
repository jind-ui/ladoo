//! Middleware system.
//!
//! Middleware functions wrap the request/response pipeline. Each middleware
//! receives a [`Context`] and a [`Next`], and can inspect the request,
//! call `next.run(ctx)` to continue the chain, and then inspect or modify
//! the response.
//!
//! # Writing Middleware
//!
//! Any async function with the right signature works:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! async fn logger(ctx: Context, next: Next) -> Result<Response> {
//!     let method = ctx.method().to_string();
//!     let path = ctx.path().to_string();
//!     let start = std::time::Instant::now();
//!     let resp = next.run(ctx).await?;
//!     println!("{method} {path} → {} ({}ms)",
//!         resp.status(), start.elapsed().as_millis());
//!     Ok(resp)
//! }
//!
//! App::new().use_mw(logger).get("/", handler);
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::context::Context;
use crate::error::Error;
use crate::handler::Handler;
use crate::response::Response;

/// A middleware that wraps the request/response pipeline.
///
/// Implementors receive an owned [`Context`] and a [`Next`] that
/// continues the chain. The blanket implementation covers all async
/// functions matching the standard middleware signature, so you rarely
/// need to implement this trait manually.
pub trait Middleware: Send + Sync {
    /// Process the request context and optionally call `next.run()`.
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>>;
}

/// Blanket implementation: any `Fn(Context, Next) -> Future<Output = Result<Response, Error>>`
/// is a Middleware.
impl<F, Fut> Middleware for F
where
    F: Fn(Context, Next) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Response, Error>> + Send + 'static,
{
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        Box::pin((self)(ctx, next))
    }
}

/// Continuation handle for the middleware chain.
///
/// Calling [`run`](Next::run) invokes the next middleware in the chain,
/// or the final handler if no middleware remains. Each `Next` is
/// consumed on use — the chain can only advance forward.
pub struct Next {
    middleware: Arc<[Arc<dyn Middleware>]>,
    handler: Arc<dyn Handler>,
    index: usize,
}

impl Next {
    /// Create a new `Next` from a middleware stack and handler.
    pub(crate) fn new(middleware: Arc<[Arc<dyn Middleware>]>, handler: Arc<dyn Handler>) -> Self {
        Self {
            middleware,
            handler,
            index: 0,
        }
    }

    /// Continue the middleware chain.
    ///
    /// Calls the next middleware, or the handler if the chain is exhausted.
    /// The `Context` is consumed — copy any request info you need before
    /// calling this method.
    pub fn run(
        self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        Box::pin(async move {
            if self.index < self.middleware.len() {
                let mw = self.middleware[self.index].clone();
                let next = Next {
                    middleware: self.middleware,
                    handler: self.handler,
                    index: self.index + 1,
                };
                mw.call(ctx, next).await
            } else {
                let request = ctx.into_request();
                Ok(self.handler.call(request).await)
            }
        })
    }
}

/// Execute a middleware chain followed by a handler.
///
/// This is the internal entry point used by the server. `handler` is
/// wrapped in an `Arc` by the caller (the router stores routes this way),
/// so no unsafe lifetime tricks are needed to build the [`Next`] chain.
pub(crate) async fn run_middleware_chain(
    middleware: &[Arc<dyn Middleware>],
    handler: Arc<dyn Handler>,
    ctx: Context,
) -> Result<Response, Error> {
    if middleware.is_empty() {
        let request = ctx.into_request();
        return Ok(handler.call(request).await);
    }

    let mw_arc: Arc<[Arc<dyn Middleware>]> = middleware.to_vec().into();
    let next = Next::new(mw_arc, handler);
    next.run(ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::handler::IntoHandler;
    use crate::request::Request;
    use http::Method;

    #[tokio::test]
    async fn next_calls_handler_when_no_middleware() {
        let handler: Arc<dyn Handler> = (|_req: Request| "hello").into_handler().into();
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&[], handler, ctx).await;
        let resp = result.unwrap();
        assert_eq!(resp.body_bytes(), b"hello");
    }

    #[tokio::test]
    async fn single_middleware_wraps_handler() {
        async fn add_header(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Test", "added");
            Ok(resp)
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "hello").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(add_header)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        let resp = result.unwrap();
        assert_eq!(resp.body_bytes(), b"hello");
        assert_eq!(
            resp.headers().get("X-Test").unwrap().to_str().unwrap(),
            "added"
        );
    }

    #[tokio::test]
    async fn middleware_ordering_outer_to_inner() {
        async fn outer(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Order", "outer");
            Ok(resp)
        }

        async fn inner(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            // inner runs AFTER outer's next.run(), so inner's header is set first
            // then outer overwrites — but we can append instead
            let existing = resp
                .headers()
                .get("X-Order")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            let value = if existing.is_empty() {
                "inner".to_string()
            } else {
                format!("{existing},inner")
            };
            resp.set_header("X-Order", &value);
            Ok(resp)
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "hello").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(outer), Arc::new(inner)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        let resp = result.unwrap();
        // Inner's set_header runs first (closer to handler), then outer overwrites
        assert_eq!(
            resp.headers().get("X-Order").unwrap().to_str().unwrap(),
            "outer"
        );
    }

    #[tokio::test]
    async fn middleware_can_short_circuit() {
        async fn blocker(_ctx: Context, _next: Next) -> Result<Response, crate::error::Error> {
            Err(crate::error::Error::unauthorized("blocked"))
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "unreachable").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(blocker)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn middleware_can_read_request_info() {
        async fn method_check(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let method = ctx.method().to_string();
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Method", &method);
            Ok(resp)
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(method_check)];
        let ctx = Context::new(Request::test(Method::POST, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        let resp = result.unwrap();
        assert_eq!(
            resp.headers().get("X-Method").unwrap().to_str().unwrap(),
            "POST"
        );
    }

    #[tokio::test]
    async fn middleware_error_propagates() {
        async fn failing_mw(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let resp = next.run(ctx).await?;
            let _ = resp;
            Err(crate::error::Error::internal("middleware failed"))
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(failing_mw)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn three_middleware_chain() {
        async fn mw1(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW1", "yes");
            Ok(resp)
        }
        async fn mw2(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW2", "yes");
            Ok(resp)
        }
        async fn mw3(ctx: Context, next: Next) -> Result<Response, crate::error::Error> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW3", "yes");
            Ok(resp)
        }

        let handler: Arc<dyn Handler> = (|_req: Request| "ok").into_handler().into();
        let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(mw1), Arc::new(mw2), Arc::new(mw3)];
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let result = run_middleware_chain(&mw, handler, ctx).await;
        let resp = result.unwrap();
        assert!(resp.headers().contains_key("X-MW1"));
        assert!(resp.headers().contains_key("X-MW2"));
        assert!(resp.headers().contains_key("X-MW3"));
    }
}
