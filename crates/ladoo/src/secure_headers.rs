//! Secure HTTP response headers middleware.
//!
//! [`SecureHeaders`] sets common security headers on every response.
//! Register it as middleware for sensible defaults:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! App::new()
//!     .use_mw(SecureHeaders::default())
//!     .get("/", |_: Request| "Hello");
//! ```
//!
//! Each header can be customized or disabled via the builder:
//!
//! ```rust,ignore
//! App::new().use_mw(
//!     SecureHeaders::new()
//!         .hsts("max-age=31536000")
//!         .x_frame_options(None)
//! )
//! ```

use http::HeaderValue;

/// Middleware that sets security-related HTTP headers on every response.
///
/// Use [`SecureHeaders::default()`] for sane defaults, or build a custom
/// configuration with the builder methods. Pass `None` to any builder
/// method to disable that header entirely.
///
/// Handler-set headers are never overwritten — if the handler already set
/// a header, `SecureHeaders` leaves it alone.
///
/// # Default Headers
///
/// | Header | Default |
/// |---|---|
/// | `Strict-Transport-Security` | `max-age=63072000; includeSubDomains` |
/// | `X-Content-Type-Options` | `nosniff` |
/// | `X-Frame-Options` | `DENY` |
/// | `Content-Security-Policy` | `default-src 'self'` |
/// | `Referrer-Policy` | `strict-origin-when-cross-origin` |
/// | `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` |
#[derive(Debug, Clone)]
pub struct SecureHeaders {
    hsts: Option<HeaderValue>,
    x_content_type_options: Option<HeaderValue>,
    x_frame_options: Option<HeaderValue>,
    content_security_policy: Option<HeaderValue>,
    referrer_policy: Option<HeaderValue>,
    permissions_policy: Option<HeaderValue>,
}

impl Default for SecureHeaders {
    fn default() -> Self {
        Self {
            hsts: Some(HeaderValue::from_static("max-age=63072000; includeSubDomains")),
            x_content_type_options: Some(HeaderValue::from_static("nosniff")),
            x_frame_options: Some(HeaderValue::from_static("DENY")),
            content_security_policy: Some(HeaderValue::from_static("default-src 'self'")),
            referrer_policy: Some(HeaderValue::from_static("strict-origin-when-cross-origin")),
            permissions_policy: Some(HeaderValue::from_static("camera=(), microphone=(), geolocation=()")),
        }
    }
}

impl SecureHeaders {
    /// Create a new `SecureHeaders` with all defaults enabled.
    ///
    /// Equivalent to [`SecureHeaders::default()`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the `Strict-Transport-Security` header.
    ///
    /// Pass a string to set a custom value, or `None` to disable.
    pub fn hsts(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.hsts = value.into().map(HeaderValue::from_static);
        self
    }

    /// Set the `X-Content-Type-Options` header.
    pub fn x_content_type_options(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.x_content_type_options = value.into().map(HeaderValue::from_static);
        self
    }

    /// Set the `X-Frame-Options` header.
    pub fn x_frame_options(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.x_frame_options = value.into().map(HeaderValue::from_static);
        self
    }

    /// Set the `Content-Security-Policy` header.
    pub fn content_security_policy(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.content_security_policy = value.into().map(HeaderValue::from_static);
        self
    }

    /// Set the `Referrer-Policy` header.
    pub fn referrer_policy(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.referrer_policy = value.into().map(HeaderValue::from_static);
        self
    }

    /// Set the `Permissions-Policy` header.
    pub fn permissions_policy(mut self, value: impl Into<Option<&'static str>>) -> Self {
        self.permissions_policy = value.into().map(HeaderValue::from_static);
        self
    }
}

use std::future::Future;
use std::pin::Pin;

use crate::context::Context;
use crate::error::Result;
use crate::middleware::{Middleware, Next};
use crate::response::Response;

impl Middleware for SecureHeaders {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
        let headers = [
            ("strict-transport-security", self.hsts.clone()),
            ("x-content-type-options", self.x_content_type_options.clone()),
            ("x-frame-options", self.x_frame_options.clone()),
            ("content-security-policy", self.content_security_policy.clone()),
            ("referrer-policy", self.referrer_policy.clone()),
            ("permissions-policy", self.permissions_policy.clone()),
        ];

        Box::pin(async move {
            let mut resp = next.run(ctx).await?;
            for (name, value) in headers {
                if let Some(val) = value {
                    if !resp.headers().contains_key(name) {
                        resp.set_header(name, val.to_str().unwrap_or_default());
                    }
                }
            }
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sets_all_six_headers() {
        let sh = SecureHeaders::default();
        assert_eq!(sh.hsts.as_ref().unwrap().to_str().unwrap(), "max-age=63072000; includeSubDomains");
        assert_eq!(sh.x_content_type_options.as_ref().unwrap().to_str().unwrap(), "nosniff");
        assert_eq!(sh.x_frame_options.as_ref().unwrap().to_str().unwrap(), "DENY");
        assert_eq!(sh.content_security_policy.as_ref().unwrap().to_str().unwrap(), "default-src 'self'");
        assert_eq!(sh.referrer_policy.as_ref().unwrap().to_str().unwrap(), "strict-origin-when-cross-origin");
        assert_eq!(sh.permissions_policy.as_ref().unwrap().to_str().unwrap(), "camera=(), microphone=(), geolocation=()");
    }

    #[test]
    fn new_equals_default() {
        let new = SecureHeaders::new();
        let def = SecureHeaders::default();
        assert_eq!(format!("{:?}", new), format!("{:?}", def));
    }

    #[test]
    fn builder_overrides_hsts() {
        let sh = SecureHeaders::new().hsts("max-age=31536000");
        assert_eq!(sh.hsts.as_ref().unwrap().to_str().unwrap(), "max-age=31536000");
    }

    #[test]
    fn builder_disables_header_with_none() {
        let sh = SecureHeaders::new().x_frame_options(None);
        assert!(sh.x_frame_options.is_none());
    }

    #[test]
    fn builder_overrides_x_content_type_options() {
        let sh = SecureHeaders::new().x_content_type_options("custom");
        assert_eq!(
            sh.x_content_type_options.as_ref().unwrap().to_str().unwrap(),
            "custom"
        );
    }

    #[test]
    fn builder_overrides_content_security_policy() {
        let sh = SecureHeaders::new().content_security_policy("default-src 'none'");
        assert_eq!(
            sh.content_security_policy.as_ref().unwrap().to_str().unwrap(),
            "default-src 'none'"
        );
    }

    #[test]
    fn multiple_builders_compose() {
        let sh = SecureHeaders::new()
            .hsts("max-age=0")
            .referrer_policy("no-referrer")
            .permissions_policy(None);
        assert_eq!(sh.hsts.as_ref().unwrap().to_str().unwrap(), "max-age=0");
        assert_eq!(sh.referrer_policy.as_ref().unwrap().to_str().unwrap(), "no-referrer");
        assert!(sh.permissions_policy.is_none());
        // Untouched headers keep defaults
        assert_eq!(sh.x_content_type_options.as_ref().unwrap().to_str().unwrap(), "nosniff");
    }

    use crate::app::App;
    use crate::request::Request;
    use http::StatusCode;

    #[tokio::test]
    async fn middleware_sets_all_default_headers() {
        let client = App::test()
            .use_mw(SecureHeaders::default())
            .get("/", |_req: Request| "ok")
            .into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.header("strict-transport-security"), Some("max-age=63072000; includeSubDomains"));
        assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(resp.header("x-frame-options"), Some("DENY"));
        assert_eq!(resp.header("content-security-policy"), Some("default-src 'self'"));
        assert_eq!(resp.header("referrer-policy"), Some("strict-origin-when-cross-origin"));
        assert_eq!(resp.header("permissions-policy"), Some("camera=(), microphone=(), geolocation=()"));
    }

    #[tokio::test]
    async fn handler_set_header_wins() {
        async fn set_csp(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("Content-Security-Policy", "default-src 'none'");
            Ok(resp)
        }

        let client = App::test()
            .use_mw(SecureHeaders::default())
            .use_mw(set_csp)
            .get("/", |_req: Request| "ok")
            .into_client();
        let resp = client.get("/").send().await;
        // Handler (via inner middleware) set CSP — SecureHeaders must not overwrite
        assert_eq!(resp.header("content-security-policy"), Some("default-src 'none'"));
        // Other defaults still applied
        assert_eq!(resp.header("x-frame-options"), Some("DENY"));
    }

    #[tokio::test]
    async fn disabled_header_not_set() {
        let client = App::test()
            .use_mw(SecureHeaders::new().x_frame_options(None))
            .get("/", |_req: Request| "ok")
            .into_client();
        let resp = client.get("/").send().await;
        assert!(resp.header("x-frame-options").is_none());
        // Other defaults still present
        assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
    }

    #[tokio::test]
    async fn applies_to_error_responses() {
        let client = App::test()
            .use_mw(SecureHeaders::default())
            .get("/fail", |_req: Request| {
                std::result::Result::<&str, crate::error::Error>::Err(
                    crate::error::Error::bad_request("nope"),
                )
            })
            .into_client();
        let resp = client.get("/fail").send().await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
    }
}
