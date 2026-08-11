//! Route registration and matching.
//!
//! The [`Router`] stores routes as `(method, path_pattern, Arc<dyn Handler>)`
//! and matches incoming requests by comparing path segments. Static segments
//! must match exactly; `:param` segments capture the corresponding value;
//! `*name` wildcard segments capture all remaining path segments joined
//! with `/`. Handlers are stored behind an `Arc` (rather than a `Box`) so a
//! matched route's handler can be shared with a [`Next`](crate::middleware::Next)
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

    #[test]
    fn find_wildcard_route() {
        let mut router = Router::new();
        router.add(Method::GET, "/assets/*path", dummy_handler());

        let m = router.find(&Method::GET, "/assets/css/style.css").unwrap();
        assert_eq!(m.params, vec![("path".into(), "css/style.css".into())]);
    }

    #[test]
    #[should_panic(expected = "wildcard segment '*path' must be the last segment")]
    fn wildcard_not_last_segment_panics() {
        let mut router = Router::new();
        router.add(Method::GET, "/files/*path/edit", dummy_handler());
    }

    #[test]
    #[should_panic(expected = "wildcard segment must have a name")]
    fn unnamed_wildcard_panics() {
        let mut router = Router::new();
        router.add(Method::GET, "/files/*", dummy_handler());
    }

    #[test]
    fn wildcard_captures_deeply_nested_path() {
        let mut router = Router::new();
        router.add(Method::GET, "/files/*path", dummy_handler());

        let m = router.find(&Method::GET, "/files/a/b/c/d.txt").unwrap();
        assert_eq!(m.params, vec![("path".into(), "a/b/c/d.txt".into())]);
    }

    #[test]
    fn wildcard_captures_empty_when_no_trailing_segments() {
        let mut router = Router::new();
        router.add(Method::GET, "/assets/*path", dummy_handler());

        let m = router.find(&Method::GET, "/assets").unwrap();
        assert_eq!(m.params, vec![("path".into(), "".into())]);
    }

    #[test]
    fn wildcard_captures_empty_with_trailing_slash() {
        let mut router = Router::new();
        router.add(Method::GET, "/assets/*path", dummy_handler());

        let m = router.find(&Method::GET, "/assets/").unwrap();
        assert_eq!(m.params, vec![("path".into(), "".into())]);
    }

    #[test]
    fn global_catch_all_wildcard() {
        let mut router = Router::new();
        router.add(Method::GET, "/*catchall", dummy_handler());

        let m = router.find(&Method::GET, "/any/nested/path").unwrap();
        assert_eq!(
            m.params,
            vec![("catchall".into(), "any/nested/path".into())],
        );
    }

    #[test]
    fn global_catch_all_matches_root() {
        let mut router = Router::new();
        router.add(Method::GET, "/*catchall", dummy_handler());

        let m = router.find(&Method::GET, "/").unwrap();
        assert_eq!(m.params, vec![("catchall".into(), "".into())]);
    }

    #[test]
    fn param_before_wildcard() {
        let mut router = Router::new();
        router.add(Method::GET, "/api/:version/*rest", dummy_handler());

        let m = router.find(&Method::GET, "/api/v2/users/42").unwrap();
        assert_eq!(
            m.params,
            vec![
                ("version".into(), "v2".into()),
                ("rest".into(), "users/42".into()),
            ],
        );
    }

    #[test]
    fn static_route_beats_wildcard() {
        let mut router = Router::new();
        router.add(Method::GET, "/*path", dummy_handler());
        router.add(Method::GET, "/health", dummy_handler());

        let m = router.find(&Method::GET, "/health").unwrap();
        assert!(m.params.is_empty());
    }

    #[test]
    fn param_route_beats_wildcard() {
        let mut router = Router::new();
        router.add(Method::GET, "/*path", dummy_handler());
        router.add(Method::GET, "/users/:id", dummy_handler());

        let m = router.find(&Method::GET, "/users/42").unwrap();
        assert_eq!(m.params, vec![("id".into(), "42".into())]);
    }

    #[test]
    fn param_route_beats_wildcard_at_same_depth() {
        let mut router = Router::new();
        router.add(Method::GET, "/*catchall", dummy_handler());
        router.add(Method::GET, "/:id", dummy_handler());

        let m = router.find(&Method::GET, "/42").unwrap();
        assert_eq!(m.params, vec![("id".into(), "42".into())]);
    }

    #[test]
    fn static_prefix_beats_wildcard_prefix() {
        let mut router = Router::new();
        router.add(Method::GET, "/assets/*path", dummy_handler());
        router.add(
            Method::GET,
            "/assets/favicon.ico",
            dummy_handler(),
        );

        let m = router.find(&Method::GET, "/assets/favicon.ico").unwrap();
        assert!(m.params.is_empty());
    }

    #[test]
    fn wildcard_no_match_wrong_prefix() {
        let mut router = Router::new();
        router.add(Method::GET, "/assets/*path", dummy_handler());

        assert!(router.find(&Method::GET, "/other/file.js").is_none());
    }

    #[test]
    fn merge_from_preserves_wildcard_routes() {
        let sub = Router::new().get("/static/*path", dummy_handler_fn);
        let mut main = Router::new();
        main.merge_from("/cdn", sub);

        let m = main.find(&Method::GET, "/cdn/static/js/app.js").unwrap();
        assert_eq!(m.params, vec![("path".into(), "js/app.js".into())]);
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

/// A path segment pattern — either a literal string, a named parameter, or
/// a wildcard catch-all.
#[derive(Debug, Clone)]
enum Segment {
    /// Matches a specific string exactly (e.g., `users`).
    Static(String),
    /// Captures any string and binds it to a name (e.g., `:id` → `id`).
    Param(String),
    /// Captures all remaining path segments, joined with `/`.
    /// Must be the last segment in the pattern (e.g., `*path`).
    Wildcard(String),
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
/// Routes are checked in registration order. Precedence: static segments
/// beat parameter segments, which beat wildcard segments. Register
/// specific routes before catch-all wildcards for predictable behavior.
///
/// # Path Patterns
///
/// - `/users` — static segment, matches exactly
/// - `/users/:id` — parameter segment, captures the value
/// - `/assets/*path` — wildcard segment, captures all remaining segments
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
/// router.add(Method::GET, "/files/*path", (|_req: Request| "file").into_handler());
///
/// assert!(router.find(&Method::GET, "/").is_some());
/// assert!(router.find(&Method::GET, "/users/42").is_some());
/// assert!(router.find(&Method::GET, "/files/docs/readme.md").is_some());
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
    /// Path patterns support static segments (`/users`), named parameters
    /// (`/users/:id`), and wildcard catch-all segments (`/assets/*path`).
    /// Parameters are prefixed with `:`, wildcards with `*`.
    ///
    /// # Panics
    ///
    /// Panics if a wildcard segment is not the last segment in the pattern,
    /// or if a wildcard has no name (bare `*`).
    pub fn add(&mut self, method: Method, path: &str, handler: Box<dyn Handler>) {
        let segments = Self::parse_path(path);
        self.routes.push(Route {
            method,
            segments,
            handler: Arc::from(handler),
            middleware: Vec::new(),
        });
    }

    /// Find a handler matching the given method and path.
    ///
    /// Returns `None` if no route matches. Precedence when multiple routes
    /// could match: static segments beat parameters, which beat wildcards
    /// — even when the competing routes have the same static-segment count
    /// (e.g. `/:id` beats `/*catchall` for the path `/42`).
    pub fn find(&self, method: &Method, path: &str) -> Option<RouteMatch<'_>> {
        let path_segments = Self::split_path(path);

        // Prefer more static segments, then prefer non-wildcard routes.
        let mut best_match: Option<(&Route, PathParams)> = None;
        let mut best_score: (usize, bool) = (0, true);

        for route in &self.routes {
            if route.method != *method {
                continue;
            }

            if let Some((params, static_count, has_wildcard)) =
                Self::match_segments(&route.segments, &path_segments)
            {
                let score = (static_count, !has_wildcard);
                if best_match.is_none() || score > best_score {
                    best_match = Some((route, params));
                    best_score = score;
                }
            }
        }

        best_match.map(|(route, params)| RouteMatch {
            handler: route.handler.clone(),
            params,
            middleware: &route.middleware,
        })
    }

    /// Returns `true` if any registered route matches `path`, regardless
    /// of HTTP method.
    ///
    /// Used to distinguish "path exists but method isn't handled" (e.g. a
    /// preflight `OPTIONS` request for a path that only registers `GET`)
    /// from a genuinely unknown path. Middleware such as [`Cors`](crate::cors::Cors)
    /// needs to run in the former case even though no route matched, but
    /// truly unmatched paths should skip middleware and return a plain 404.
    pub(crate) fn path_exists(&self, path: &str) -> bool {
        let path_segments = Self::split_path(path);
        self.routes
            .iter()
            .any(|route| Self::match_segments(&route.segments, &path_segments).is_some())
    }

    /// Parse a path pattern like `/users/:id` or `/assets/*path` into segments.
    ///
    /// # Panics
    ///
    /// Panics if a wildcard segment (`*name`) is not the last segment in the
    /// pattern, or if a wildcard has no name (bare `*`).
    fn parse_path(path: &str) -> Vec<Segment> {
        let parts: Vec<&str> = Self::split_path(path);
        let mut segments = Vec::with_capacity(parts.len());

        for (i, s) in parts.iter().enumerate() {
            if let Some(name) = s.strip_prefix('*') {
                if name.is_empty() {
                    panic!("wildcard segment must have a name (e.g., '*path')");
                }
                if i != parts.len() - 1 {
                    panic!(
                        "wildcard segment '*{name}' must be the last segment in path pattern"
                    );
                }
                segments.push(Segment::Wildcard(name.to_string()));
            } else if let Some(name) = s.strip_prefix(':') {
                segments.push(Segment::Param(name.to_string()));
            } else {
                segments.push(Segment::Static(s.to_string()));
            }
        }

        segments
    }

    /// Split a path string into non-empty segments.
    fn split_path(path: &str) -> Vec<&str> {
        path.split('/')
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Try to match path segments against a route pattern.
    ///
    /// Returns extracted params plus a priority score used to rank
    /// competing matches: the count of static segments, and whether the
    /// pattern ends in a wildcard. Static segments matter most; among
    /// routes with the same static count, non-wildcard routes (params)
    /// outrank wildcard routes.
    fn match_segments(
        pattern: &[Segment],
        path: &[&str],
    ) -> Option<(PathParams, usize, bool)> {
        let has_wildcard = matches!(pattern.last(), Some(Segment::Wildcard(_)));

        if has_wildcard {
            // Wildcard: path must have at least (pattern.len() - 1) segments
            if path.len() < pattern.len() - 1 {
                return None;
            }
        } else {
            // No wildcard: exact segment count required
            if pattern.len() != path.len() {
                return None;
            }
        }

        let mut params = Vec::new();
        let mut static_count = 0;

        for (i, segment) in pattern.iter().enumerate() {
            match segment {
                Segment::Static(expected) => {
                    if path.get(i).is_none_or(|v| v != expected) {
                        return None;
                    }
                    static_count += 1;
                }
                Segment::Param(name) => {
                    let value = path.get(i)?;
                    params.push((name.clone(), (*value).to_string()));
                }
                Segment::Wildcard(name) => {
                    let tail = &path[i..];
                    params.push((name.clone(), tail.join("/")));
                }
            }
        }

        Some((params, static_count, has_wildcard))
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

    /// Attach an authentication provider to this route group.
    ///
    /// All routes in this router will require authentication via the
    /// given provider. On success, the authenticated user is stored
    /// in per-request state and available via [`Auth<T>`](crate::auth::Auth).
    ///
    /// This is sugar for `.use_mw(AuthGuardMiddleware::new(provider))`.
    /// For custom auth flows, use [`.use_mw()`](Router::use_mw) directly.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// let jwt = JwtAuth::<Claims>::hs256(b"secret");
    /// App::new()
    ///     .group("/api", |g| g.guard(jwt).get("/me", handler))
    /// ```
    pub fn guard<P: crate::auth::AuthProvider>(self, provider: P) -> Self {
        self.use_mw(crate::auth::AuthGuardMiddleware::new(provider))
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
            Segment::Wildcard(v) => format!("/*{v}"),
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
