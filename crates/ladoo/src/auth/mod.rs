//! Authentication and authorization.
//!
//! This module provides the [`AuthProvider`] trait for implementing
//! authentication, the [`Auth`] extractor for type-safe user access
//! in handlers, and the [`HasRole`] trait for role-based authorization.
//!
//! # Architecture
//!
//! Auth uses a two-phase pattern:
//!
//! 1. **Middleware phase (async):** [`AuthProvider::authenticate`] runs
//!    in middleware, storing the user via `ctx.provide(user)`.
//! 2. **Extractor phase (sync):** `Auth<T>`'s [`FromRequest`] impl does a
//!    per-request state lookup — zero-cost beyond the HashMap get.
//!
//! An [`AuthProvider`] is wrapped in guard middleware and registered with
//! [`.use_mw()`](crate::router::Router::use_mw) to protect a route or
//! route group.

pub mod providers;
pub mod rbac;

use std::fmt;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http::StatusCode;

use crate::context::Context;
use crate::error::Error;
use crate::extract::FromRequest;
use crate::middleware::{Middleware, Next};
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// Authentication/authorization error.
///
/// Returned by [`AuthProvider::authenticate`] when credentials are
/// missing, invalid, or expired. `Missing`, `Invalid`, and `Expired`
/// produce 401 responses; `Forbidden` produces 403.
#[derive(Debug, Clone)]
pub enum AuthError {
    /// No credentials were provided.
    Missing,
    /// Credentials were present but invalid.
    Invalid(String),
    /// Credentials expired (e.g., JWT `exp` claim in the past).
    Expired,
    /// Authenticated but not authorized for this action.
    Forbidden(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Missing => write!(f, "Authentication required"),
            AuthError::Invalid(msg) => write!(f, "Authentication failed: {msg}"),
            AuthError::Expired => write!(f, "Token expired"),
            AuthError::Forbidden(msg) => write!(f, "Forbidden: {msg}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let status = match &self {
            AuthError::Missing | AuthError::Invalid(_) | AuthError::Expired => {
                StatusCode::UNAUTHORIZED
            }
            AuthError::Forbidden(_) => StatusCode::FORBIDDEN,
        };
        let message = self.to_string();

        #[cfg(feature = "json")]
        {
            let body = serde_json::json!({
                "error": message,
                "status": status.as_u16(),
            });
            let mut headers = http::HeaderMap::new();
            headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            Response::new(status, headers, Bytes::from(body.to_string()))
        }

        #[cfg(not(feature = "json"))]
        {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            Response::new(status, headers, Bytes::from(message))
        }
    }
}

/// Authenticated user extractor.
///
/// Extracts the authenticated user from per-request state. Returns
/// 401 if no auth middleware has stored a user of type `T`.
///
/// Use [`Option<Auth<T>>`] for routes where authentication is optional.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// async fn profile(user: Auth<User>) -> String {
///     format!("Hello, {}", user.name)
/// }
/// ```
#[derive(Debug)]
pub struct Auth<T>(pub T);

impl<T> Deref for Auth<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone + Send + Sync + 'static> FromRequest for Auth<T> {
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        match req.per_request().get::<T>() {
            Some(user) => Ok(Auth(user.clone())),
            None => Err(Error::unauthorized("Authentication required").into_response()),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> FromRequest for Option<Auth<T>> {
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        Ok(req.per_request().get::<T>().map(|u| Auth(u.clone())))
    }
}

/// Authenticate requests.
///
/// Implement this trait to define how your application verifies
/// credentials (JWT, API keys, sessions, etc.). The associated
/// [`User`](AuthProvider::User) type is what handlers receive via
/// [`Auth<T>`].
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
/// use async_trait::async_trait;
///
/// struct MyAuth { /* ... */ }
///
/// #[async_trait]
/// impl AuthProvider for MyAuth {
///     type User = MyUser;
///     async fn authenticate(&self, req: &Request) -> Result<MyUser, AuthError> {
///         let token = req.headers().get("Authorization")
///             .and_then(|v| v.to_str().ok())
///             .ok_or(AuthError::Missing)?;
///         // validate token, look up user...
///         Ok(MyUser { /* ... */ })
///     }
/// }
/// ```
#[async_trait]
pub trait AuthProvider: Send + Sync + 'static {
    /// The user type this provider authenticates to.
    type User: Clone + Send + Sync + 'static;

    /// Validate credentials from the request and return the authenticated user.
    async fn authenticate(&self, req: &Request) -> Result<Self::User, AuthError>;
}

/// Middleware that runs an [`AuthProvider`] and stores the user.
///
/// On success, stores the user via `ctx.provide(user)` so handlers can
/// extract it with [`Auth<T>`]. On failure, short-circuits with the
/// error response.
///
/// Constructed automatically by [`Router::guard()`](crate::router::Router::guard);
/// most applications never build this directly.
pub(crate) struct AuthGuardMiddleware<P: AuthProvider> {
    provider: Arc<P>,
}

impl<P: AuthProvider> AuthGuardMiddleware<P> {
    /// Wrap a provider in guard middleware.
    pub(crate) fn new(provider: P) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }
}

impl<P: AuthProvider> Middleware for AuthGuardMiddleware<P> {
    fn call(
        &self,
        mut ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        let provider = self.provider.clone();
        Box::pin(async move {
            match provider.authenticate(ctx.request()).await {
                Ok(user) => {
                    ctx.provide(user);
                    next.run(ctx).await
                }
                Err(e) => Ok(e.into_response()),
            }
        })
    }
}

/// A user type that carries role information.
///
/// Implement this on your user/claims struct so that authorization
/// guards can check roles for access control.
///
/// # Examples
///
/// ```
/// use ladoo::auth::HasRole;
///
/// struct User { role_list: Vec<String> }
///
/// impl HasRole for User {
///     fn roles(&self) -> &[String] { &self.role_list }
/// }
/// ```
pub trait HasRole {
    /// Returns the roles assigned to this user.
    fn roles(&self) -> &[String];
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    #[test]
    fn auth_error_missing_display() {
        let err = AuthError::Missing;
        assert_eq!(format!("{err}"), "Authentication required");
    }

    #[test]
    fn auth_error_invalid_display() {
        let err = AuthError::Invalid("bad token".into());
        assert_eq!(format!("{err}"), "Authentication failed: bad token");
    }

    #[test]
    fn auth_error_expired_display() {
        let err = AuthError::Expired;
        assert_eq!(format!("{err}"), "Token expired");
    }

    #[test]
    fn auth_error_forbidden_display() {
        let err = AuthError::Forbidden("requires role 'admin'".into());
        assert_eq!(format!("{err}"), "Forbidden: requires role 'admin'");
    }

    #[test]
    fn auth_error_missing_is_401() {
        let resp = AuthError::Missing.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn auth_error_invalid_is_401() {
        let resp = AuthError::Invalid("bad".into()).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn auth_error_expired_is_401() {
        let resp = AuthError::Expired.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn auth_error_forbidden_is_403() {
        let resp = AuthError::Forbidden("nope".into()).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(feature = "json")]
    #[test]
    fn auth_error_json_body() {
        let resp = AuthError::Missing.into_response();
        let body: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["error"], "Authentication required");
        assert_eq!(body["status"], 401);
    }

    #[cfg(not(feature = "json"))]
    #[test]
    fn auth_error_plain_text_body() {
        let resp = AuthError::Missing.into_response();
        assert_eq!(resp.body_bytes(), b"Authentication required");
    }

    #[test]
    fn auth_error_is_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<AuthError>();
    }

    #[test]
    fn auth_extracts_user_from_per_request_state() {
        let mut req = Request::test(Method::GET, "/");
        req.provide(42_u32);
        let auth = Auth::<u32>::from_request(&mut req).unwrap();
        assert_eq!(*auth, 42);
    }

    #[test]
    fn auth_deref_to_inner() {
        let auth = Auth("hello".to_string());
        let s: &String = &auth;
        assert_eq!(s, "hello");
    }

    #[test]
    fn auth_returns_401_when_no_user() {
        let mut req = Request::test(Method::GET, "/");
        let result = Auth::<u32>::from_request(&mut req);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn option_auth_returns_some_when_present() {
        let mut req = Request::test(Method::GET, "/");
        req.provide(42_u32);
        let opt = Option::<Auth<u32>>::from_request(&mut req).unwrap();
        assert_eq!(opt.unwrap().0, 42);
    }

    #[test]
    fn option_auth_returns_none_when_absent() {
        let mut req = Request::test(Method::GET, "/");
        let opt = Option::<Auth<u32>>::from_request(&mut req).unwrap();
        assert!(opt.is_none());
    }

    #[derive(Clone, Debug, PartialEq)]
    struct TestUser {
        name: String,
    }

    struct AlwaysAuth {
        user: TestUser,
    }

    #[async_trait]
    impl AuthProvider for AlwaysAuth {
        type User = TestUser;
        async fn authenticate(&self, _req: &Request) -> Result<TestUser, AuthError> {
            Ok(self.user.clone())
        }
    }

    struct AlwaysDeny;

    #[async_trait]
    impl AuthProvider for AlwaysDeny {
        type User = TestUser;
        async fn authenticate(&self, _req: &Request) -> Result<TestUser, AuthError> {
            Err(AuthError::Invalid("denied".into()))
        }
    }

    #[tokio::test]
    async fn auth_guard_middleware_stores_user_on_success() {
        use crate::handler::IntoHandler;

        let provider = AlwaysAuth {
            user: TestUser { name: "Alice".into() },
        };
        let mw = AuthGuardMiddleware::new(provider);

        let handler: Arc<dyn crate::handler::Handler> = (|mut req: Request| {
            let user = Auth::<TestUser>::from_request(&mut req).unwrap();
            user.name.clone()
        })
        .into_handler()
        .into();
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.body_bytes(), b"Alice");
    }

    #[tokio::test]
    async fn auth_guard_middleware_returns_401_on_failure() {
        use crate::handler::IntoHandler;

        let mw = AuthGuardMiddleware::new(AlwaysDeny);

        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();
        let ctx = Context::new(Request::test(Method::GET, "/"));
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await;
        // AuthGuardMiddleware returns Ok(error_response), not Err.
        let resp = resp.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    impl HasRole for TestUser {
        fn roles(&self) -> &[String] {
            &[]
        }
    }

    #[derive(Clone)]
    struct RoledUser {
        roles: Vec<String>,
    }

    impl HasRole for RoledUser {
        fn roles(&self) -> &[String] {
            &self.roles
        }
    }

    #[test]
    fn has_role_returns_roles() {
        let user = RoledUser {
            roles: vec!["admin".into(), "user".into()],
        };
        assert_eq!(user.roles(), &["admin".to_string(), "user".to_string()]);
    }

    #[test]
    fn has_role_empty() {
        let user = TestUser { name: "Bob".into() };
        assert!(user.roles().is_empty());
    }

    #[tokio::test]
    async fn guard_integrates_with_router() {
        use crate::app::App;

        let provider = AlwaysAuth {
            user: TestUser { name: "Alice".into() },
        };

        let client = App::test()
            .group("/api", |r| {
                r.guard(provider)
                    .get("/me", |mut req: Request| {
                        let user = Auth::<TestUser>::from_request(&mut req).unwrap();
                        user.name.clone()
                    })
            })
            .into_client();

        let resp = client.get("/api/me").send().await;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text(), "Alice");
    }

    #[tokio::test]
    async fn guard_rejects_unauthenticated() {
        use crate::app::App;

        let client = App::test()
            .group("/api", |r| {
                r.guard(AlwaysDeny)
                    .get("/me", |_req: Request| "unreachable")
            })
            .into_client();

        let resp = client.get("/api/me").send().await;
        assert_eq!(resp.status(), 401);
    }
}
