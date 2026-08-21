//! API key authentication provider.
//!
//! Authenticates requests by looking up a header value in a
//! pre-configured set of API keys. Each key maps to a user value.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! let auth = ApiKeyAuth::new()
//!     .key("secret-key-1", User { name: "Alice".into() })
//!     .key("secret-key-2", User { name: "Bob".into() });
//!
//! App::new()
//!     .group("/api", |g| g.guard(auth).get("/me", handler))
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use subtle::ConstantTimeEq;

use crate::auth::{AuthError, AuthProvider};
use crate::request::Request;

/// API key authentication provider.
///
/// Authenticates by matching a request header against a set of known
/// keys. Defaults to the `X-API-Key` header.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::auth::providers::ApiKeyAuth;
///
/// let auth = ApiKeyAuth::new()
///     .header("Authorization")
///     .key("secret-key", my_user);
/// ```
pub struct ApiKeyAuth<U: Clone + Send + Sync + 'static> {
    keys: HashMap<String, U>,
    header_name: String,
}

impl<U: Clone + Send + Sync + 'static> ApiKeyAuth<U> {
    /// Create a new API key authenticator using the `X-API-Key` header.
    pub fn new() -> Self {
        Self {
            keys: HashMap::new(),
            header_name: "X-API-Key".to_string(),
        }
    }

    /// Set the header name to read the API key from.
    pub fn header(mut self, name: &str) -> Self {
        self.header_name = name.to_string();
        self
    }

    /// Register an API key that maps to a user.
    pub fn key(mut self, key: impl Into<String>, user: U) -> Self {
        self.keys.insert(key.into(), user);
        self
    }
}

impl<U: Clone + Send + Sync + 'static> Default for ApiKeyAuth<U> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<U: Clone + Send + Sync + 'static> AuthProvider for ApiKeyAuth<U> {
    type User = U;

    async fn authenticate(&self, req: &Request) -> Result<U, AuthError> {
        let key = req
            .headers()
            .get(&self.header_name)
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::Missing)?;

        let key_bytes = key.as_bytes();
        self.keys
            .iter()
            .find(|(k, _)| k.len() == key_bytes.len() && bool::from(k.as_bytes().ct_eq(key_bytes)))
            .map(|(_, user)| user.clone())
            .ok_or_else(|| AuthError::Invalid("Invalid API key".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    #[derive(Clone, Debug, PartialEq)]
    struct User {
        name: String,
    }

    #[tokio::test]
    async fn valid_key_returns_user() {
        let auth = ApiKeyAuth::new().key(
            "key-1",
            User {
                name: "Alice".into(),
            },
        );
        let mut headers = http::HeaderMap::new();
        headers.insert("X-API-Key", http::HeaderValue::from_static("key-1"));
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        assert_eq!(
            result.unwrap(),
            User {
                name: "Alice".into()
            }
        );
    }

    #[tokio::test]
    async fn missing_header_returns_missing() {
        let auth = ApiKeyAuth::<User>::new().key(
            "key-1",
            User {
                name: "Alice".into(),
            },
        );
        let req = Request::test(Method::GET, "/");
        let result = auth.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Missing));
    }

    #[tokio::test]
    async fn unknown_key_returns_invalid() {
        let auth = ApiKeyAuth::new().key(
            "key-1",
            User {
                name: "Alice".into(),
            },
        );
        let mut headers = http::HeaderMap::new();
        headers.insert("X-API-Key", http::HeaderValue::from_static("wrong"));
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn custom_header_name() {
        let auth = ApiKeyAuth::new()
            .header("Authorization")
            .key("token-abc", User { name: "Bob".into() });
        let mut headers = http::HeaderMap::new();
        headers.insert("Authorization", http::HeaderValue::from_static("token-abc"));
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        assert_eq!(result.unwrap(), User { name: "Bob".into() });
    }

    #[tokio::test]
    async fn api_key_rejects_invalid_key_with_generic_message() {
        let auth = ApiKeyAuth::new().key(
            "key-abc-123",
            User {
                name: "Alice".into(),
            },
        );
        let mut headers = http::HeaderMap::new();
        headers.insert("X-API-Key", http::HeaderValue::from_static("wrong-key"));
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        match result.unwrap_err() {
            AuthError::Invalid(msg) => assert_eq!(msg, "Invalid API key"),
            other => panic!("Expected AuthError::Invalid, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn multiple_keys() {
        let auth = ApiKeyAuth::new()
            .key(
                "key-1",
                User {
                    name: "Alice".into(),
                },
            )
            .key("key-2", User { name: "Bob".into() });
        let mut headers = http::HeaderMap::new();
        headers.insert("X-API-Key", http::HeaderValue::from_static("key-2"));
        let req = Request::test_with_headers(Method::GET, "/", headers);
        assert_eq!(
            auth.authenticate(&req).await.unwrap(),
            User { name: "Bob".into() }
        );
    }
}
