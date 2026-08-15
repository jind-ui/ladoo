//! Integration tests for HTTP/2 cleartext (h2c) support.
//!
//! These exercise the `hyper_util::server::conn::auto::Builder` swap in
//! `crates/ladoo/src/server.rs`, which lets the server negotiate HTTP/2
//! over plain TCP (h2c, via prior knowledge) while remaining backwards
//! compatible with HTTP/1.1 clients.

use ladoo::prelude::*;

/// Verify the server handles HTTP/2 cleartext (h2c) via prior knowledge.
#[tokio::test]
async fn h2c_prior_knowledge_responds() {
    let server = App::test()
        .get("/hello", |_: Request| "h2 works")
        .spawn()
        .await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let resp = client
        .get(server.url("/hello"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.version(), reqwest::Version::HTTP_2);
    assert_eq!(resp.text().await.unwrap(), "h2 works");
}

/// Verify HTTP/1.1 clients still work (backwards compatibility).
#[tokio::test]
async fn http1_still_works_with_auto_builder() {
    let server = App::test()
        .get("/hello", |_: Request| "http1 ok")
        .spawn()
        .await;

    let client = reqwest::Client::builder()
        .http1_only()
        .build()
        .unwrap();

    let resp = client
        .get(server.url("/hello"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.version(), reqwest::Version::HTTP_11);
    assert_eq!(resp.text().await.unwrap(), "http1 ok");
}

/// Verify h2c works with POST bodies.
#[tokio::test]
async fn h2c_with_post_body() {
    let server = App::test()
        .post("/echo", |req: Request| {
            String::from_utf8_lossy(req.body()).to_string()
        })
        .spawn()
        .await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let resp = client
        .post(server.url("/echo"))
        .body("hello h2")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.version(), reqwest::Version::HTTP_2);
    assert_eq!(resp.text().await.unwrap(), "hello h2");
}

/// Verify middleware works over HTTP/2.
#[tokio::test]
async fn h2c_with_middleware() {
    use ladoo::context::Context;
    use ladoo::error::Result;
    use ladoo::middleware::Next;
    use ladoo::response::Response;

    async fn tag(ctx: Context, next: Next) -> Result<Response> {
        let mut resp = next.run(ctx).await?;
        resp.set_header("X-Protocol", "h2c");
        Ok(resp)
    }

    let server = App::test()
        .use_mw(tag)
        .get("/mw", |_: Request| "middleware ok")
        .spawn()
        .await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let resp = client.get(server.url("/mw")).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("X-Protocol").unwrap().to_str().unwrap(),
        "h2c"
    );
    assert_eq!(resp.text().await.unwrap(), "middleware ok");
}

/// Verify server starts and stops cleanly with h2c client.
#[tokio::test]
async fn h2c_server_lifecycle() {
    let server = App::test().get("/", |_: Request| "ok").spawn().await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let resp = client.get(server.url("/")).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.version(), reqwest::Version::HTTP_2);
    // TestServer drops cleanly — exercises shutdown with h2c connection
}

/// Verify HTTP/2 multiplexing — concurrent requests on one connection.
#[tokio::test]
async fn h2c_multiplexing() {
    let server = App::test()
        .get("/a", |_: Request| "response a")
        .get("/b", |_: Request| "response b")
        .get("/c", |_: Request| "response c")
        .spawn()
        .await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let (a, b, c) = tokio::join!(
        client.get(server.url("/a")).send(),
        client.get(server.url("/b")).send(),
        client.get(server.url("/c")).send(),
    );

    let a = a.unwrap();
    let b = b.unwrap();
    let c = c.unwrap();

    assert_eq!(a.version(), reqwest::Version::HTTP_2);
    assert_eq!(b.version(), reqwest::Version::HTTP_2);
    assert_eq!(c.version(), reqwest::Version::HTTP_2);
    assert_eq!(a.text().await.unwrap(), "response a");
    assert_eq!(b.text().await.unwrap(), "response b");
    assert_eq!(c.text().await.unwrap(), "response c");
}
