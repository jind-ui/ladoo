//! Cross-Origin Resource Sharing (CORS) middleware.
//!
//! [`Cors`] handles preflight `OPTIONS` requests and adds the appropriate
//! `Access-Control-*` headers to responses. Use [`Cors::permissive()`]
//! for development or build a custom policy with the builder methods.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! // Allow everything (development)
//! App::new()
//!     .use_mw(Cors::permissive())
//!     .get("/api/data", handler);
//!
//! // Custom policy (production)
//! App::new()
//!     .use_mw(
//!         Cors::new()
//!             .allow_origin("https://myapp.com")
//!             .allow_methods([Method::GET, Method::POST])
//!             .allow_headers(["Content-Type", "Authorization"])
//!             .max_age(std::time::Duration::from_secs(3600))
//!     )
//!     .get("/api/data", handler);
//! ```

use std::time::Duration;

use http::Method;

/// Allowed origin configuration.
#[derive(Debug, Clone)]
enum AllowedOrigins {
    /// Any origin matches (`Access-Control-Allow-Origin: *`).
    Any,
    /// Only origins in this list match.
    List(Vec<String>),
}

/// CORS middleware that handles preflight requests and sets
/// `Access-Control-*` headers on responses.
///
/// Use [`Cors::permissive()`] for development (allows everything) or
/// build a custom policy with [`Cors::new()`] and the builder methods.
///
/// Preflight `OPTIONS` requests are auto-intercepted and receive a
/// `204 No Content` response with CORS headers — the handler is never
/// called. Non-preflight cross-origin requests get CORS headers added
/// to the normal response.
///
/// Handler-set `Access-Control-*` headers are never overwritten.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// App::new()
///     .use_mw(Cors::permissive())
///     .get("/api/data", handler);
/// ```
#[derive(Debug, Clone)]
pub struct Cors {
    origins: AllowedOrigins,
    methods: Vec<Method>,
    headers: Vec<String>,
    expose_headers: Vec<String>,
    max_age: Option<Duration>,
    allow_credentials: bool,
}

impl Cors {
    /// Create a restrictive CORS policy.
    ///
    /// No origins are allowed by default. Use the builder methods to
    /// add allowed origins, methods, and headers.
    pub fn new() -> Self {
        Self {
            origins: AllowedOrigins::List(Vec::new()),
            methods: vec![Method::GET, Method::HEAD, Method::OPTIONS],
            headers: vec!["content-type".to_string()],
            expose_headers: Vec::new(),
            max_age: None,
            allow_credentials: false,
        }
    }

    /// Create a fully permissive CORS policy.
    ///
    /// Allows any origin, all standard methods, any header, no
    /// credentials. Suitable for development and public APIs.
    pub fn permissive() -> Self {
        Self {
            origins: AllowedOrigins::Any,
            methods: vec![
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::PATCH,
                Method::HEAD,
                Method::OPTIONS,
            ],
            headers: Vec::new(), // wildcard — allow any
            expose_headers: Vec::new(),
            max_age: None,
            allow_credentials: false,
        }
    }

    /// Add an allowed origin.
    ///
    /// Call multiple times to allow several origins. The `Origin`
    /// request header is matched against this list; only matching
    /// origins are echoed back in the response.
    pub fn allow_origin(mut self, origin: &str) -> Self {
        match &mut self.origins {
            AllowedOrigins::Any => {
                self.origins = AllowedOrigins::List(vec![origin.to_string()]);
            }
            AllowedOrigins::List(list) => {
                list.push(origin.to_string());
            }
        }
        self
    }

    /// Set the allowed HTTP methods for CORS requests.
    ///
    /// Replaces any previously configured methods.
    pub fn allow_methods<I>(mut self, methods: I) -> Self
    where
        I: IntoIterator<Item = Method>,
    {
        self.methods = methods.into_iter().collect();
        self
    }

    /// Set the allowed request headers for CORS requests.
    ///
    /// Replaces any previously configured headers. Header names are
    /// compared case-insensitively.
    pub fn allow_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.headers = headers
            .into_iter()
            .map(|h| h.into().to_lowercase())
            .collect();
        self
    }

    /// Set headers the browser is allowed to read from the response.
    ///
    /// These appear in `Access-Control-Expose-Headers`.
    pub fn expose_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.expose_headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// Set how long the browser caches preflight results.
    ///
    /// Sent as `Access-Control-Max-Age` in seconds.
    pub fn max_age(mut self, duration: Duration) -> Self {
        self.max_age = Some(duration);
        self
    }

    /// Allow credentials (cookies, HTTP auth) in CORS requests.
    ///
    /// When enabled, `Access-Control-Allow-Origin` echoes the specific
    /// origin instead of `*` (required by the CORS spec).
    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.allow_credentials = allow;
        self
    }

    /// Check whether the given origin is allowed by this policy.
    pub(crate) fn is_origin_allowed(&self, origin: &str) -> bool {
        match &self.origins {
            AllowedOrigins::Any => true,
            AllowedOrigins::List(list) => list.iter().any(|o| o == origin),
        }
    }

    /// Whether credentials are allowed.
    pub(crate) fn credentials(&self) -> bool {
        self.allow_credentials
    }
}

impl Default for Cors {
    fn default() -> Self {
        Self::new()
    }
}

use std::future::Future;
use std::pin::Pin;

use crate::context::Context;
use crate::error::Result;
use crate::middleware::{Middleware, Next};
use crate::response::Response;

impl Middleware for Cors {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
        let cors = self.clone();

        Box::pin(async move {
            let origin = ctx
                .headers()
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            // No Origin header → not a CORS request, pass through
            let origin = match origin {
                Some(o) => o,
                None => return next.run(ctx).await,
            };

            // Check if this origin is allowed
            if !cors.is_origin_allowed(&origin) {
                return next.run(ctx).await;
            }

            // Check if this is a preflight request
            let is_preflight = ctx.method() == Method::OPTIONS
                && ctx.headers().contains_key("access-control-request-method");

            if is_preflight {
                let mut resp = Response::empty(http::StatusCode::NO_CONTENT);
                cors.set_preflight_headers(&mut resp, &origin);
                return Ok(resp);
            }

            // Normal CORS request — call the handler, then add headers
            let mut resp = next.run(ctx).await?;
            cors.set_cors_headers(&mut resp, &origin);
            Ok(resp)
        })
    }
}

impl Cors {
    fn set_preflight_headers(&self, resp: &mut Response, origin: &str) {
        self.set_origin_header(resp, origin);

        let methods: Vec<&str> = self.methods.iter().map(|m| m.as_str()).collect();
        resp.set_header("access-control-allow-methods", &methods.join(", "));

        if self.headers.is_empty() {
            // Permissive mode: allow any header
            resp.set_header("access-control-allow-headers", "*");
        } else {
            resp.set_header("access-control-allow-headers", &self.headers.join(", "));
        }

        if let Some(max_age) = self.max_age {
            resp.set_header("access-control-max-age", &max_age.as_secs().to_string());
        }

        if self.credentials() {
            resp.set_header("access-control-allow-credentials", "true");
        }
    }

    fn set_cors_headers(&self, resp: &mut Response, origin: &str) {
        if !resp.headers().contains_key("access-control-allow-origin") {
            self.set_origin_header(resp, origin);
        }

        if self.credentials() && !resp.headers().contains_key("access-control-allow-credentials")
        {
            resp.set_header("access-control-allow-credentials", "true");
        }

        if !self.expose_headers.is_empty()
            && !resp.headers().contains_key("access-control-expose-headers")
        {
            resp.set_header(
                "access-control-expose-headers",
                &self.expose_headers.join(", "),
            );
        }
    }

    fn set_origin_header(&self, resp: &mut Response, origin: &str) {
        match &self.origins {
            AllowedOrigins::Any if !self.credentials() => {
                resp.set_header("access-control-allow-origin", "*");
            }
            _ => {
                resp.set_header("access-control-allow-origin", origin);
                resp.set_header("vary", "origin");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_restrictive_cors() {
        let cors = Cors::new();
        // No origins allowed by default
        assert!(!cors.is_origin_allowed("https://example.com"));
    }

    #[test]
    fn permissive_allows_any_origin() {
        let cors = Cors::permissive();
        assert!(cors.is_origin_allowed("https://anything.com"));
    }

    #[test]
    fn allow_origin_adds_specific_origin() {
        let cors = Cors::new().allow_origin("https://myapp.com");
        assert!(cors.is_origin_allowed("https://myapp.com"));
        assert!(!cors.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn allow_origin_supports_multiple() {
        let cors = Cors::new()
            .allow_origin("https://app.com")
            .allow_origin("https://admin.com");
        assert!(cors.is_origin_allowed("https://app.com"));
        assert!(cors.is_origin_allowed("https://admin.com"));
        assert!(!cors.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn allow_credentials_defaults_to_false() {
        let cors = Cors::new();
        assert!(!cors.credentials());
    }

    #[test]
    fn allow_credentials_can_be_enabled() {
        let cors = Cors::new().allow_credentials(true);
        assert!(cors.credentials());
    }

    #[test]
    fn max_age_defaults_to_none() {
        let cors = Cors::new();
        // `max_age` is a builder setter (Duration -> Self), so the getter
        // for the stored value is a direct field read here, matching the
        // SecureHeaders test pattern this module follows.
        assert!(cors.max_age.is_none());
    }

    #[test]
    fn max_age_can_be_set() {
        let cors = Cors::new().max_age(Duration::from_secs(3600));
        assert_eq!(cors.max_age, Some(Duration::from_secs(3600)));
    }

    use crate::app::App;
    use crate::request::Request;
    use http::StatusCode;

    #[tokio::test]
    async fn preflight_returns_204_with_cors_headers() {
        let client = App::test()
            .use_mw(Cors::permissive())
            .get("/api/data", |_: Request| "ok")
            .into_client();

        let resp = client
            .request(Method::OPTIONS, "/api/data")
            .header("origin", "https://example.com")
            .header("access-control-request-method", "GET")
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
        assert!(resp.header("access-control-allow-methods").is_some());
        assert_eq!(resp.text(), "");
    }

    #[tokio::test]
    async fn non_cors_request_passes_through_without_headers() {
        let client = App::test()
            .use_mw(Cors::permissive())
            .get("/", |_: Request| "hello")
            .into_client();

        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text(), "hello");
        assert!(resp.header("access-control-allow-origin").is_none());
    }

    #[tokio::test]
    async fn cors_request_adds_headers_to_response() {
        let client = App::test()
            .use_mw(Cors::permissive())
            .get("/api", |_: Request| "data")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://example.com")
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text(), "data");
        assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
    }

    #[tokio::test]
    async fn specific_origin_echoed_back() {
        let client = App::test()
            .use_mw(Cors::new().allow_origin("https://myapp.com"))
            .get("/api", |_: Request| "data")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://myapp.com")
            .send()
            .await;

        assert_eq!(
            resp.header("access-control-allow-origin"),
            Some("https://myapp.com")
        );
    }

    #[tokio::test]
    async fn unmatched_origin_gets_no_cors_headers() {
        let client = App::test()
            .use_mw(Cors::new().allow_origin("https://myapp.com"))
            .get("/api", |_: Request| "data")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://evil.com")
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.header("access-control-allow-origin").is_none());
    }

    #[tokio::test]
    async fn credentials_echoes_origin_not_star() {
        let client = App::test()
            .use_mw(
                Cors::new()
                    .allow_origin("https://myapp.com")
                    .allow_credentials(true),
            )
            .get("/api", |_: Request| "ok")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://myapp.com")
            .send()
            .await;

        assert_eq!(
            resp.header("access-control-allow-origin"),
            Some("https://myapp.com")
        );
        assert_eq!(
            resp.header("access-control-allow-credentials"),
            Some("true")
        );
    }

    #[tokio::test]
    async fn vary_origin_set_for_specific_origins() {
        let client = App::test()
            .use_mw(Cors::new().allow_origin("https://myapp.com"))
            .get("/api", |_: Request| "ok")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://myapp.com")
            .send()
            .await;

        assert_eq!(resp.header("vary"), Some("origin"));
    }

    #[tokio::test]
    async fn max_age_sent_on_preflight() {
        let client = App::test()
            .use_mw(Cors::permissive().max_age(Duration::from_secs(3600)))
            .get("/api", |_: Request| "ok")
            .into_client();

        let resp = client
            .request(Method::OPTIONS, "/api")
            .header("origin", "https://example.com")
            .header("access-control-request-method", "GET")
            .send()
            .await;

        assert_eq!(resp.header("access-control-max-age"), Some("3600"));
    }

    #[tokio::test]
    async fn expose_headers_sent_on_cors_response() {
        let client = App::test()
            .use_mw(Cors::permissive().expose_headers(["X-Request-Id", "X-Total-Count"]))
            .get("/api", |_: Request| "ok")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://example.com")
            .send()
            .await;

        let exposed = resp.header("access-control-expose-headers").unwrap();
        assert!(exposed.contains("X-Request-Id"));
        assert!(exposed.contains("X-Total-Count"));
    }

    #[tokio::test]
    async fn handler_set_cors_header_not_overwritten() {
        async fn set_origin(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("Access-Control-Allow-Origin", "https://custom.com");
            Ok(resp)
        }

        let client = App::test()
            .use_mw(Cors::permissive())
            .use_mw(set_origin)
            .get("/api", |_: Request| "ok")
            .into_client();

        let resp = client
            .get("/api")
            .header("origin", "https://example.com")
            .send()
            .await;

        assert_eq!(
            resp.header("access-control-allow-origin"),
            Some("https://custom.com")
        );
    }

    #[tokio::test]
    async fn scoped_cors_only_applies_to_group() {
        let client = App::test()
            .group("/api", |g| {
                g.use_mw(Cors::permissive())
                    .get("/data", |_: Request| "api data")
            })
            .get("/page", |_: Request| "page")
            .into_client();

        let api_resp = client
            .get("/api/data")
            .header("origin", "https://example.com")
            .send()
            .await;
        assert!(api_resp.header("access-control-allow-origin").is_some());

        let page_resp = client
            .get("/page")
            .header("origin", "https://example.com")
            .send()
            .await;
        assert!(page_resp.header("access-control-allow-origin").is_none());
    }
}
