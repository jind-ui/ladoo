//! Route registration and matching.
//!
//! The [`Router`] stores routes as `(method, path_pattern, Arc<dyn Handler>)`
//! and matches incoming requests by comparing path segments. Static segments
//! must match exactly; `:param` segments capture the corresponding value.
//! Handlers are stored behind an `Arc` (rather than a `Box`) so a matched
//! route's handler can be shared with a [`Next`](crate::middleware::Next)
//! for middleware chain execution without borrowing from the router.
//!
//! # Examples
//!
//! ```
//! use ladoo::router::Router;
//! use ladoo::handler::IntoHandler;
//! use ladoo::request::Request;
//! use http::Method;
//!
//! let mut router = Router::new();
//! router.add(Method::GET, "/users/:id", (|_req: Request| "user").into_handler());
//!
//! let m = router.find(&Method::GET, "/users/42").unwrap();
//! assert_eq!(m.params[0], ("id".to_string(), "42".to_string()));
//! ```

use std::sync::Arc;

use http::Method;

use crate::handler::{Handler, IntoHandler};
use crate::middleware::Middleware;
use crate::request::PathParams;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::IntoHandler;
    use crate::request::Request;
    use http::StatusCode;

    fn dummy_handler() -> Box<dyn Handler> {
        (|_req: Request| "ok").into_handler()
    }

    fn dummy_handler_fn(_req: Request) -> &'static str {
        "ok"
    }

    #[test]
    fn find_static_route() {
        let mut router = Router::new();
        router.add(Method::GET, "/", dummy_handler());

        let m = router.find(&Method::GET, "/");
        assert!(m.is_some());
        assert!(m.unwrap().params.is_empty());
    }

    #[test]
    fn find_returns_none_for_unregistered_path() {
        let mut router = Router::new();
        router.add(Method::GET, "/", dummy_handler());

        assert!(router.find(&Method::GET, "/unknown").is_none());
    }

    #[test]
    fn find_returns_none_for_wrong_method() {
        let mut router = Router::new();
        router.add(Method::GET, "/users", dummy_handler());

        assert!(router.find(&Method::POST, "/users").is_none());
    }

    #[test]
    fn find_with_path_param() {
        let mut router = Router::new();
        router.add(Method::GET, "/users/:id", dummy_handler());

        let m = router.find(&Method::GET, "/users/42").unwrap();
        assert_eq!(m.params, vec![("id".into(), "42".into())]);
    }

    #[test]
    fn find_with_multiple_params() {
        let mut router = Router::new();
        router.add(
            Method::GET,
            "/users/:user_id/posts/:post_id",
            dummy_handler(),
        );

        let m = router
            .find(&Method::GET, "/users/1/posts/99")
            .unwrap();
        assert_eq!(
            m.params,
            vec![
                ("user_id".into(), "1".into()),
                ("post_id".into(), "99".into()),
            ]
        );
    }

    #[test]
    fn find_distinguishes_methods() {
        let mut router = Router::new();
        router.add(Method::GET, "/users", dummy_handler());
        router.add(Method::POST, "/users", dummy_handler());

        assert!(router.find(&Method::GET, "/users").is_some());
        assert!(router.find(&Method::POST, "/users").is_some());
        assert!(router.find(&Method::DELETE, "/users").is_none());
    }

    #[test]
    fn find_static_before_param() {
        let mut router = Router::new();
        router.add(Method::GET, "/users/me", dummy_handler());
        router.add(Method::GET, "/users/:id", dummy_handler());

        // Static route should match first
        let m = router.find(&Method::GET, "/users/me").unwrap();
        assert!(m.params.is_empty());
    }

    #[test]
    fn find_does_not_match_different_segment_count() {
        let mut router = Router::new();
        router.add(Method::GET, "/users/:id", dummy_handler());

        assert!(router.find(&Method::GET, "/users").is_none());
        assert!(router.find(&Method::GET, "/users/1/extra").is_none());
    }

    #[test]
    fn find_root_path() {
        let mut router = Router::new();
        router.add(Method::GET, "/", dummy_handler());

        assert!(router.find(&Method::GET, "/").is_some());
        assert!(router.find(&Method::GET, "/other").is_none());
    }

    #[test]
    fn find_with_trailing_slash() {
        let mut router = Router::new();
        router.add(Method::GET, "/users", dummy_handler());

        // Trailing slash should still match
        assert!(router.find(&Method::GET, "/users/").is_some());
    }

    #[tokio::test]
    async fn matched_handler_executes() {
        let mut router = Router::new();
        router.add(
            Method::GET,
            "/hello",
            (|_req: Request| "Hello!").into_handler(),
        );

        let m = router.find(&Method::GET, "/hello").unwrap();
        let req = Request::test(Method::GET, "/hello");
        let resp = m.handler.call(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"Hello!");
    }

    #[test]
    fn get_convenience_method() {
        let router = Router::new().get("/", dummy_handler_fn);
        assert!(router.find(&Method::GET, "/").is_some());
    }

    #[test]
    fn post_convenience_method() {
        let router = Router::new().post("/users", dummy_handler_fn);
        assert!(router.find(&Method::POST, "/users").is_some());
    }

    #[test]
    fn put_convenience_method() {
        let router = Router::new().put("/users/:id", dummy_handler_fn);
        assert!(router.find(&Method::PUT, "/users/1").is_some());
    }

    #[test]
    fn delete_convenience_method() {
        let router = Router::new().delete("/users/:id", dummy_handler_fn);
        assert!(router.find(&Method::DELETE, "/users/1").is_some());
    }

    #[test]
    fn patch_convenience_method() {
        let router = Router::new().patch("/users/:id", dummy_handler_fn);
        assert!(router.find(&Method::PATCH, "/users/1").is_some());
    }

    #[test]
    fn merge_from_adds_prefixed_routes() {
        let sub = Router::new()
            .get("/", dummy_handler_fn)
            .get("/:id", dummy_handler_fn);
        let mut main = Router::new();
        main.merge_from("/users", sub);
        assert!(main.find(&Method::GET, "/users").is_some());
        assert!(main.find(&Method::GET, "/users/42").is_some());
    }
}

/// A matched route, containing the handler, extracted path parameters, and
/// the route's own middleware stack.
pub struct RouteMatch<'a> {
    /// The handler to call for this route.
    pub handler: Arc<dyn Handler>,
    /// Path parameters extracted during matching (e.g., `[("id", "42")]`).
    pub params: PathParams,
    /// Middleware registered specifically for this route (e.g., via a
    /// route group). Does not include global middleware — callers combine
    /// the two.
    pub middleware: &'a [Arc<dyn Middleware>],
}

/// A path segment pattern — either a literal string or a named parameter.
#[derive(Debug, Clone)]
enum Segment {
    /// Matches a specific string exactly (e.g., `users`).
    Static(String),
    /// Captures any string and binds it to a name (e.g., `:id` → `id`).
    Param(String),
}

/// A registered route: method + path pattern + handler + middleware.
struct Route {
    method: Method,
    segments: Vec<Segment>,
    handler: Arc<dyn Handler>,
    middleware: Vec<Arc<dyn Middleware>>,
}

/// Routes HTTP requests to handlers based on method and path pattern.
///
/// Routes are checked in registration order. Static segments take priority
/// over parameter segments when both could match — register specific routes
/// before parameterized ones for predictable behavior.
///
/// # Examples
///
/// ```
/// use ladoo::router::Router;
/// use ladoo::handler::IntoHandler;
/// use ladoo::request::Request;
/// use http::Method;
///
/// let mut router = Router::new();
/// router.add(Method::GET, "/", (|_req: Request| "home").into_handler());
/// router.add(Method::GET, "/users/:id", (|_req: Request| "user").into_handler());
///
/// assert!(router.find(&Method::GET, "/").is_some());
/// assert!(router.find(&Method::GET, "/users/42").is_some());
/// assert!(router.find(&Method::GET, "/missing").is_none());
/// ```
pub struct Router {
    routes: Vec<Route>,
    group_middleware: Vec<Arc<dyn Middleware>>,
}

impl Router {
    /// Create an empty router with no routes.
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            group_middleware: Vec::new(),
        }
    }

    /// Register a route with the given HTTP method, path pattern, and handler.
    ///
    /// Path patterns support static segments (`/users`) and named parameters
    /// (`/users/:id`). Parameters are prefixed with `:`.
    pub fn add(&mut self, method: Method, path: &str, handler: Box<dyn Handler>) {
        let segments = Self::parse_path(path);
        self.routes.push(Route {
            method,
            segments,
            handler: Arc::from(handler),
            middleware: Vec::new(),
        });
    }

    /// Register a route along with middleware specific to it.
    ///
    /// Used internally by route groups, which attach a shared middleware
    /// stack to every route registered within the group.
    // Not yet called — route groups (a later task) are the intended caller.
    #[allow(dead_code)]
    pub(crate) fn add_with_middleware(
        &mut self,
        method: Method,
        path: &str,
        handler: Box<dyn Handler>,
        middleware: Vec<Arc<dyn Middleware>>,
    ) {
        let segments = Self::parse_path(path);
        self.routes.push(Route {
            method,
            segments,
            handler: Arc::from(handler),
            middleware,
        });
    }

    /// Find a handler matching the given method and path.
    ///
    /// Returns `None` if no route matches. When multiple routes could match,
    /// routes with static segments are preferred over parameterized ones.
    pub fn find(&self, method: &Method, path: &str) -> Option<RouteMatch<'_>> {
        let path_segments = Self::split_path(path);

        // Prefer static matches over param matches
        let mut best_match: Option<(&Route, PathParams)> = None;
        let mut best_static_count = 0;

        for route in &self.routes {
            if route.method != *method {
                continue;
            }

            if let Some((params, static_count)) =
                Self::match_segments(&route.segments, &path_segments)
            {
                if best_match.is_none() || static_count > best_static_count {
                    best_match = Some((route, params));
                    best_static_count = static_count;
                }
            }
        }

        best_match.map(|(route, params)| RouteMatch {
            handler: route.handler.clone(),
            params,
            middleware: &route.middleware,
        })
    }

    /// Parse a path pattern like `/users/:id` into segments.
    fn parse_path(path: &str) -> Vec<Segment> {
        Self::split_path(path)
            .into_iter()
            .map(|s| {
                if let Some(name) = s.strip_prefix(':') {
                    Segment::Param(name.to_string())
                } else {
                    Segment::Static(s.to_string())
                }
            })
            .collect()
    }

    /// Split a path string into non-empty segments.
    fn split_path(path: &str) -> Vec<&str> {
        path.split('/')
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Try to match path segments against a route pattern.
    /// Returns extracted params and a count of static matches (for priority).
    fn match_segments(
        pattern: &[Segment],
        path: &[&str],
    ) -> Option<(PathParams, usize)> {
        if pattern.len() != path.len() {
            return None;
        }

        let mut params = Vec::new();
        let mut static_count = 0;

        for (segment, value) in pattern.iter().zip(path.iter()) {
            match segment {
                Segment::Static(expected) => {
                    if expected != value {
                        return None;
                    }
                    static_count += 1;
                }
                Segment::Param(name) => {
                    params.push((name.clone(), (*value).to_string()));
                }
            }
        }

        Some((params, static_count))
    }

    /// Register a handler for GET requests.
    pub fn get<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.add(Method::GET, path, handler.into_handler());
        self
    }

    /// Register a handler for POST requests.
    pub fn post<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.add(Method::POST, path, handler.into_handler());
        self
    }

    /// Register a handler for PUT requests.
    pub fn put<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.add(Method::PUT, path, handler.into_handler());
        self
    }

    /// Register a handler for DELETE requests.
    pub fn delete<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.add(Method::DELETE, path, handler.into_handler());
        self
    }

    /// Register a handler for PATCH requests.
    pub fn patch<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.add(Method::PATCH, path, handler.into_handler());
        self
    }

    /// Add middleware to every route in this router.
    ///
    /// Applies to all routes registered on this router, whether they were
    /// added before or after this call — this is what lets a route group
    /// declare its middleware and its routes in any order:
    ///
    /// ```rust,ignore
    /// Router::new()
    ///     .use_mw(auth)
    ///     .get("/dashboard", handler)
    ///     .get("/settings", handler)
    /// ```
    ///
    /// The middleware is resolved when this router is merged into a parent
    /// (via [`App::group`](crate::app::App::group) or
    /// [`App::mount`](crate::app::App::mount)).
    pub fn use_mw<MW: Middleware + 'static>(mut self, mw: MW) -> Self {
        self.group_middleware.push(Arc::new(mw));
        self
    }

    /// Merge another router's routes into this one, prepending a prefix.
    ///
    /// Each route's path segments are reconstructed with the prefix
    /// prepended and re-parsed, so parameter segments (`:id`) are
    /// preserved. Per-route middleware and any router-wide middleware
    /// added via [`Router::use_mw`] on `other` are both carried over.
    pub fn merge_from(&mut self, prefix: &str, other: Router) {
        let group_middleware = other.group_middleware;
        for route in other.routes {
            let prefixed_path = format_prefixed_path(prefix, &route.segments);
            let mut middleware = route.middleware;
            middleware.extend(group_middleware.iter().cloned());
            self.routes.push(Route {
                method: route.method,
                segments: Self::parse_path(&prefixed_path),
                handler: route.handler,
                middleware,
            });
        }
    }
}

/// Reconstruct a path string from parsed segments, prefixed with `prefix`.
fn format_prefixed_path(prefix: &str, segments: &[Segment]) -> String {
    let suffix: String = segments
        .iter()
        .map(|s| match s {
            Segment::Static(v) => format!("/{v}"),
            Segment::Param(v) => format!("/:{v}"),
        })
        .collect();
    let prefix = prefix.trim_end_matches('/');
    if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}{suffix}")
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
