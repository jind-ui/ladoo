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

use crate::handler::Handler;
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
}

/// A matched route, containing the handler and extracted path parameters.
pub struct RouteMatch {
    /// The handler to call for this route.
    pub handler: Arc<dyn Handler>,
    /// Path parameters extracted during matching (e.g., `[("id", "42")]`).
    pub params: PathParams,
}

/// A path segment pattern — either a literal string or a named parameter.
#[derive(Debug, Clone)]
enum Segment {
    /// Matches a specific string exactly (e.g., `users`).
    Static(String),
    /// Captures any string and binds it to a name (e.g., `:id` → `id`).
    Param(String),
}

/// A registered route: method + path pattern + handler.
struct Route {
    method: Method,
    segments: Vec<Segment>,
    handler: Arc<dyn Handler>,
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
}

impl Router {
    /// Create an empty router with no routes.
    pub fn new() -> Self {
        Self { routes: Vec::new() }
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
        });
    }

    /// Find a handler matching the given method and path.
    ///
    /// Returns `None` if no route matches. When multiple routes could match,
    /// routes with static segments are preferred over parameterized ones.
    pub fn find(&self, method: &Method, path: &str) -> Option<RouteMatch> {
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
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
