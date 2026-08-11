//! Integration tests for auth exercised through the public `App` and
//! `TestClient` API — API key authentication, optional auth, and RBAC
//! role enforcement, the way a real user would wire them up.

#![cfg(feature = "json")]

use ladoo::prelude::*;

#[derive(Clone, Debug)]
struct User {
    name: String,
    user_roles: Vec<String>,
}

impl HasRole for User {
    fn roles(&self) -> &[String] {
        &self.user_roles
    }
}

#[tokio::test]
async fn api_key_auth_valid_key() {
    let auth = ApiKeyAuth::new().key(
        "secret-key",
        User {
            name: "Alice".into(),
            user_roles: vec!["admin".into()],
        },
    );

    let client = App::test()
        .group("/api", |g| {
            g.guard(auth).get("/me", |mut req: Request| {
                let user = Auth::<User>::from_request(&mut req).unwrap();
                user.name.clone()
            })
        })
        .into_client();

    let resp = client
        .get("/api/me")
        .header("X-API-Key", "secret-key")
        .send()
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "Alice");
}

#[tokio::test]
async fn api_key_auth_missing_key_returns_401() {
    let auth = ApiKeyAuth::new().key(
        "secret-key",
        User {
            name: "Alice".into(),
            user_roles: vec![],
        },
    );

    let client = App::test()
        .group("/api", |g| {
            g.guard(auth).get("/me", |_req: Request| "unreachable")
        })
        .into_client();

    let resp = client.get("/api/me").send().await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn api_key_auth_invalid_key_returns_401() {
    let auth = ApiKeyAuth::new().key(
        "secret-key",
        User {
            name: "Alice".into(),
            user_roles: vec![],
        },
    );

    let client = App::test()
        .group("/api", |g| {
            g.guard(auth).get("/me", |_req: Request| "unreachable")
        })
        .into_client();

    let resp = client
        .get("/api/me")
        .header("X-API-Key", "wrong-key")
        .send()
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn optional_auth_returns_none_without_middleware() {
    let client = App::test()
        .get("/", |mut req: Request| {
            let user = Option::<Auth<User>>::from_request(&mut req).unwrap();
            match user {
                Some(_) => "authenticated".to_string(),
                None => "guest".to_string(),
            }
        })
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "guest");
}

#[tokio::test]
async fn require_role_passes_with_correct_role() {
    let auth = ApiKeyAuth::new().key(
        "admin-key",
        User {
            name: "Alice".into(),
            user_roles: vec!["admin".into()],
        },
    );

    let client = App::test()
        .group("/admin", |g| {
            g.guard(auth)
                .use_mw(RequireRole::<User>::new("admin"))
                .get("/stats", |_req: Request| "secret stats")
        })
        .into_client();

    let resp = client
        .get("/admin/stats")
        .header("X-API-Key", "admin-key")
        .send()
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "secret stats");
}

#[tokio::test]
async fn require_role_rejects_wrong_role() {
    let auth = ApiKeyAuth::new().key(
        "user-key",
        User {
            name: "Bob".into(),
            user_roles: vec!["user".into()],
        },
    );

    let client = App::test()
        .group("/admin", |g| {
            g.guard(auth)
                .use_mw(RequireRole::<User>::new("admin"))
                .get("/stats", |_req: Request| "unreachable")
        })
        .into_client();

    let resp = client
        .get("/admin/stats")
        .header("X-API-Key", "user-key")
        .send()
        .await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn public_routes_unaffected_by_guarded_group() {
    let auth = ApiKeyAuth::new().key(
        "key",
        User {
            name: "Alice".into(),
            user_roles: vec![],
        },
    );

    let client = App::test()
        .get("/health", |_req: Request| "ok")
        .group("/api", |g| {
            g.guard(auth).get("/me", |_req: Request| "protected")
        })
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "ok");
}
