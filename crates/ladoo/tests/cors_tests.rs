#![cfg(feature = "cors")]

use http::{Method, StatusCode};
use ladoo::prelude::*;

#[tokio::test]
async fn cors_permissive_preflight_full_flow() {
    let client = App::test()
        .use_mw(Cors::permissive())
        .get("/api/users", |_: Request| "[]")
        .post("/api/users", |_: Request| "created")
        .into_client();

    // Preflight for POST
    let resp = client
        .request(Method::OPTIONS, "/api/users")
        .header("origin", "https://frontend.com")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .send()
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
    assert!(resp
        .header("access-control-allow-methods")
        .unwrap()
        .contains("POST"));

    // Actual CORS GET
    let resp = client
        .get("/api/users")
        .header("origin", "https://frontend.com")
        .send()
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
    assert_eq!(resp.text(), "[]");
}

#[tokio::test]
async fn cors_custom_policy_rejects_wrong_origin() {
    let client = App::test()
        .use_mw(
            Cors::new()
                .allow_origin("https://trusted.com")
                .allow_methods([Method::GET])
                .allow_credentials(true),
        )
        .get("/api", |_: Request| "data")
        .into_client();

    // Trusted origin
    let resp = client
        .get("/api")
        .header("origin", "https://trusted.com")
        .send()
        .await;
    assert_eq!(
        resp.header("access-control-allow-origin"),
        Some("https://trusted.com")
    );
    assert_eq!(
        resp.header("access-control-allow-credentials"),
        Some("true")
    );

    // Untrusted origin
    let resp = client
        .get("/api")
        .header("origin", "https://evil.com")
        .send()
        .await;
    assert!(resp.header("access-control-allow-origin").is_none());
}
