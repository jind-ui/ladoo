#[cfg(feature = "cors")]
use http::Method;
use http::StatusCode;
use ladoo::prelude::*;

#[tokio::test]
async fn wrong_method_returns_405_with_allow_header() {
    let client = App::test()
        .get("/api/users", |_: Request| "list")
        .post("/api/users", |_: Request| "create")
        .into_client();

    let resp = client.delete("/api/users").send().await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(resp.header("allow"), Some("GET, POST"));
}

#[tokio::test]
async fn unknown_path_still_returns_404() {
    let client = App::test()
        .get("/api/users", |_: Request| "list")
        .into_client();

    let resp = client.get("/api/nothing").send().await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(resp.header("allow").is_none());
}

#[tokio::test]
async fn wrong_method_on_param_route_returns_405() {
    let client = App::test()
        .get("/users/:id", |_: Request| "user")
        .put("/users/:id", |_: Request| "update")
        .into_client();

    let resp = client.delete("/users/42").send().await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(resp.header("allow"), Some("GET, PUT"));
}

#[cfg(feature = "cors")]
#[tokio::test]
async fn cors_preflight_still_works_with_405_fallback() {
    let client = App::test()
        .use_mw(Cors::permissive())
        .get("/api/data", |_: Request| "data")
        .into_client();

    // OPTIONS preflight should be intercepted by CORS, not hit 405
    let resp = client
        .request(Method::OPTIONS, "/api/data")
        .header("origin", "https://example.com")
        .header("access-control-request-method", "GET")
        .send()
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
}

#[tokio::test]
async fn middleware_runs_before_405_handler() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let ran = Arc::new(AtomicBool::new(false));
    let ran_clone = ran.clone();

    let mw = move |ctx: Context, next: Next| {
        let ran = ran_clone.clone();
        async move {
            ran.store(true, Ordering::SeqCst);
            next.run(ctx).await
        }
    };

    let client = App::test()
        .use_mw(mw)
        .get("/resource", |_: Request| "ok")
        .into_client();

    let resp = client.post("/resource").send().await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(ran.load(Ordering::SeqCst), "middleware should have run");
}
