//! Role-based access control.
//!
//! Provides [`Rbac`] for configuring role-permission mappings,
//! [`RequireRole`] and [`RequirePermission`] middleware guards, and
//! the [`ResourcePolicy`] trait for per-resource authorization.
//!
//! # Examples
//!
//! ```
//! use ladoo::auth::rbac::Rbac;
//!
//! let rbac = Rbac::new()
//!     .role("viewer", &["posts:read"])
//!     .role("editor", &["posts:read", "posts:write"])
//!     .role("admin", &["*"]);
//!
//! assert!(rbac.has_permission("admin", "anything"));
//! assert!(rbac.has_permission("editor", "posts:write"));
//! assert!(!rbac.has_permission("viewer", "posts:write"));
//! ```

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use crate::auth::{AuthError, HasRole};
use crate::context::Context;
use crate::error::Error;
use crate::middleware::{Middleware, Next};
use crate::response::{IntoResponse, Response};

/// Role-based access control configuration.
///
/// Maps role names to sets of permissions. Stored via
/// [`App::provide`](crate::app::App::provide) and used by
/// [`RequirePermission`] to authorize requests.
///
/// # Wildcard Matching
///
/// - `"*"` matches any permission
/// - `"posts:*"` matches any permission starting with `"posts:"`
/// - Exact strings match exactly
///
/// # Examples
///
/// ```
/// use ladoo::auth::rbac::Rbac;
///
/// let rbac = Rbac::new()
///     .role("viewer", &["posts:read", "comments:read"])
///     .role("admin", &["*"]);
///
/// assert!(rbac.has_permission("viewer", "posts:read"));
/// assert!(!rbac.has_permission("viewer", "posts:write"));
/// assert!(rbac.has_permission("admin", "anything"));
/// ```
pub struct Rbac {
    roles: HashMap<String, HashSet<String>>,
}

impl Rbac {
    /// Create an empty RBAC configuration with no roles.
    pub fn new() -> Self {
        Self {
            roles: HashMap::new(),
        }
    }

    /// Define a role with a set of permissions.
    pub fn role(mut self, name: &str, permissions: &[&str]) -> Self {
        let perms = permissions.iter().map(|p| p.to_string()).collect();
        self.roles.insert(name.to_string(), perms);
        self
    }

    /// Check whether the given role grants the given permission.
    ///
    /// Supports wildcard matching: `"*"` matches everything,
    /// `"posts:*"` matches any permission starting with `"posts:"`.
    pub fn has_permission(&self, role: &str, permission: &str) -> bool {
        let Some(perms) = self.roles.get(role) else {
            return false;
        };
        perms.iter().any(|p| {
            if p == "*" {
                true
            } else if let Some(prefix) = p.strip_suffix(":*") {
                permission.starts_with(prefix)
                    && permission.as_bytes().get(prefix.len()) == Some(&b':')
            } else {
                p == permission
            }
        })
    }
}

impl Default for Rbac {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware that requires the authenticated user to have a specific role.
///
/// Reads the user from per-request state (must run after auth middleware).
/// Returns 401 if no user is found, 403 if the user lacks the role.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// App::new()
///     .group("/admin", |g| g
///         .guard(my_auth)
///         .use_mw(RequireRole::<MyUser>::new("admin"))
///         .get("/stats", admin_stats)
///     )
/// ```
pub struct RequireRole<U: HasRole + Send + Sync + 'static> {
    role: String,
    _user: PhantomData<U>,
}

impl<U: HasRole + Send + Sync + 'static> RequireRole<U> {
    /// Create a role guard requiring the given role.
    pub fn new(role: &str) -> Self {
        Self {
            role: role.to_string(),
            _user: PhantomData,
        }
    }
}

impl<U: HasRole + Send + Sync + 'static> Middleware for RequireRole<U> {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        let role = self.role.clone();
        Box::pin(async move {
            let user: Arc<U> = match ctx.request().per_request().get_shared::<U>() {
                Some(u) => u,
                None => return Ok(AuthError::Missing.into_response()),
            };
            if user.roles().iter().any(|r| r == &role) {
                next.run(ctx).await
            } else {
                Ok(AuthError::Forbidden(format!("requires role '{role}'")).into_response())
            }
        })
    }
}

/// Middleware that requires the authenticated user to have a specific permission.
///
/// Reads the user from per-request state and the [`Rbac`] config from
/// app state. Returns 401 if no user, 403 if no role grants the
/// permission.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// let rbac = Rbac::new()
///     .role("editor", &["posts:read", "posts:write"]);
///
/// App::new()
///     .provide(rbac)
///     .group("/api", |g| g
///         .guard(my_auth)
///         .use_mw(RequirePermission::<MyUser>::new("posts:write"))
///         .post("/posts", create_post)
///     )
/// ```
pub struct RequirePermission<U: HasRole + Send + Sync + 'static> {
    permission: String,
    _user: PhantomData<U>,
}

impl<U: HasRole + Send + Sync + 'static> RequirePermission<U> {
    /// Create a permission guard requiring the given permission.
    pub fn new(permission: &str) -> Self {
        Self {
            permission: permission.to_string(),
            _user: PhantomData,
        }
    }
}

impl<U: HasRole + Send + Sync + 'static> Middleware for RequirePermission<U> {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>> {
        let permission = self.permission.clone();
        Box::pin(async move {
            let user: Arc<U> = match ctx.request().per_request().get_shared::<U>() {
                Some(u) => u,
                None => return Ok(AuthError::Missing.into_response()),
            };

            let has_perm = {
                let rbac: Arc<Rbac> = match ctx.request().extensions().get_shared::<Rbac>() {
                    Some(rbac) => rbac,
                    None => {
                        return Ok(Error::internal(
                            "Missing state: Rbac — did you forget to call .provide()?",
                        )
                        .into_response())
                    }
                };
                user.roles()
                    .iter()
                    .any(|role| rbac.has_permission(role, &permission))
            };

            if has_perm {
                next.run(ctx).await
            } else {
                Ok(
                    AuthError::Forbidden(format!("requires permission '{permission}'"))
                        .into_response(),
                )
            }
        })
    }
}

/// Per-resource authorization policy.
///
/// Unlike [`RequireRole`] and [`RequirePermission`] (which are
/// middleware), `ResourcePolicy` is called in handlers for
/// instance-level checks ("can THIS user do THIS action on THIS
/// resource?").
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::auth::rbac::ResourcePolicy;
///
/// struct PostPolicy;
///
/// impl ResourcePolicy for PostPolicy {
///     type User = User;
///     type Resource = Post;
///
///     fn can(&self, user: &User, action: &str, resource: &Post) -> bool {
///         match action {
///             "read" => true,
///             "update" | "delete" => resource.author_id == user.id,
///             _ => false,
///         }
///     }
/// }
/// ```
pub trait ResourcePolicy: Send + Sync + 'static {
    /// The authenticated user type.
    type User;
    /// The resource being accessed.
    type Resource;

    /// Check whether the user can perform the action on the resource.
    fn can(&self, user: &Self::User, action: &str, resource: &Self::Resource) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Request;
    use http::Method;

    #[test]
    fn rbac_exact_permission() {
        let rbac = Rbac::new().role("editor", &["posts:write"]);
        assert!(rbac.has_permission("editor", "posts:write"));
        assert!(!rbac.has_permission("editor", "posts:read"));
    }

    #[test]
    fn rbac_wildcard_all() {
        let rbac = Rbac::new().role("admin", &["*"]);
        assert!(rbac.has_permission("admin", "anything"));
        assert!(rbac.has_permission("admin", "posts:write"));
    }

    #[test]
    fn rbac_namespace_wildcard() {
        let rbac = Rbac::new().role("editor", &["posts:*"]);
        assert!(rbac.has_permission("editor", "posts:read"));
        assert!(rbac.has_permission("editor", "posts:write"));
        assert!(!rbac.has_permission("editor", "comments:read"));
    }

    #[test]
    fn rbac_unknown_role() {
        let rbac = Rbac::new().role("admin", &["*"]);
        assert!(!rbac.has_permission("unknown", "anything"));
    }

    #[test]
    fn rbac_multiple_roles() {
        let rbac = Rbac::new()
            .role("viewer", &["posts:read"])
            .role("editor", &["posts:read", "posts:write"]);
        assert!(rbac.has_permission("viewer", "posts:read"));
        assert!(!rbac.has_permission("viewer", "posts:write"));
        assert!(rbac.has_permission("editor", "posts:write"));
    }

    #[test]
    fn rbac_empty_permissions() {
        let rbac = Rbac::new().role("none", &[]);
        assert!(!rbac.has_permission("none", "anything"));
    }

    #[test]
    fn rbac_default_has_no_roles() {
        let rbac = Rbac::default();
        assert!(!rbac.has_permission("anyone", "anything"));
    }

    #[derive(Clone)]
    struct TestUser {
        user_roles: Vec<String>,
    }

    impl HasRole for TestUser {
        fn roles(&self) -> &[String] {
            &self.user_roles
        }
    }

    #[tokio::test]
    async fn require_role_passes_when_user_has_role() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequireRole::<TestUser>::new("admin");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();

        let mut req = Request::test(Method::GET, "/");
        req.provide(TestUser {
            user_roles: vec!["admin".into(), "user".into()],
        });
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.body_bytes(), b"ok");
    }

    #[tokio::test]
    async fn require_role_returns_403_when_missing_role() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequireRole::<TestUser>::new("admin");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();

        let mut req = Request::test(Method::GET, "/");
        req.provide(TestUser {
            user_roles: vec!["user".into()],
        });
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn require_role_returns_401_when_no_user() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequireRole::<TestUser>::new("admin");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();

        let req = Request::test(Method::GET, "/");
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_permission_passes_when_granted() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequirePermission::<TestUser>::new("posts:write");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "ok").into_handler().into();

        let rbac = Rbac::new().role("editor", &["posts:read", "posts:write"]);

        let mut req = Request::test(Method::GET, "/");
        req.provide(TestUser {
            user_roles: vec!["editor".into()],
        });
        req.provide_test_state(rbac);
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.body_bytes(), b"ok");
    }

    #[tokio::test]
    async fn require_permission_returns_403_when_not_granted() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequirePermission::<TestUser>::new("posts:delete");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();

        let rbac = Rbac::new().role("editor", &["posts:read", "posts:write"]);

        let mut req = Request::test(Method::GET, "/");
        req.provide(TestUser {
            user_roles: vec!["editor".into()],
        });
        req.provide_test_state(rbac);
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn require_permission_returns_401_when_no_user() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequirePermission::<TestUser>::new("posts:write");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();

        let mut req = Request::test(Method::GET, "/");
        req.provide_test_state(Rbac::new().role("editor", &["posts:write"]));
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_permission_returns_500_when_rbac_missing() {
        use crate::handler::IntoHandler;
        use std::sync::Arc;

        let mw = RequirePermission::<TestUser>::new("posts:write");
        let handler: Arc<dyn crate::handler::Handler> =
            (|_req: Request| "unreachable").into_handler().into();

        let mut req = Request::test(Method::GET, "/");
        req.provide(TestUser {
            user_roles: vec!["editor".into()],
        });
        // No Rbac provided via provide_test_state.
        let ctx = Context::new(req);
        let mw_arc: Arc<[Arc<dyn Middleware>]> = vec![Arc::new(mw) as Arc<dyn Middleware>].into();
        let next = Next::new(mw_arc, handler);
        let resp = next.run(ctx).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    struct TestPolicy;

    impl ResourcePolicy for TestPolicy {
        type User = TestUser;
        type Resource = String;

        fn can(&self, _user: &TestUser, action: &str, resource: &String) -> bool {
            // Only the owner can update
            action == "read" || (action == "update" && resource == "owned-by-user")
        }
    }

    #[test]
    fn resource_policy_allows() {
        let policy = TestPolicy;
        let user = TestUser { user_roles: vec![] };
        assert!(policy.can(&user, "read", &"anything".into()));
        assert!(policy.can(&user, "update", &"owned-by-user".into()));
    }

    #[test]
    fn resource_policy_denies() {
        let policy = TestPolicy;
        let user = TestUser { user_roles: vec![] };
        assert!(!policy.can(&user, "update", &"not-owned".into()));
    }
}
