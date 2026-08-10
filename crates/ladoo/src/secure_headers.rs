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
}
