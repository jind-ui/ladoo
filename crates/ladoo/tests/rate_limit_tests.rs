#![cfg(feature = "rate-limit")]

use http::StatusCode;
use ladoo::prelude::*;
use std::time::Duration;

#[tokio::test]
async fn rate_limit_full_flow_with_recovery() {
    // A zero-duration window would appear "already expired" on every
    // subsequent check (Instant::now() always advances past it), so the
    // counter would never accumulate. Use a short-but-positive window so
    // requests fired back-to-back land in the same window, and the sleep
    // below clears it.
    let window = Duration::from_millis(50);
    let client = App::test()
        .use_mw(
            RateLimit::new()
                .limit(2)
                .window(window)
                .key(RateKey::Ip),
        )
        .get("/", |_: Request| "ok")
        .into_client();

    // Two requests allowed
    let r1 = client.get("/").peer_ip("10.0.0.1").send().await;
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(r1.header("x-ratelimit-remaining"), Some("1"));

    let r2 = client.get("/").peer_ip("10.0.0.1").send().await;
    assert_eq!(r2.status(), StatusCode::OK);
    assert_eq!(r2.header("x-ratelimit-remaining"), Some("0"));

    // Third request blocked
    let r3 = client.get("/").peer_ip("10.0.0.1").send().await;
    assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);

    // After the window expires, the counter resets
    tokio::time::sleep(window * 2).await;
    let r4 = client.get("/").peer_ip("10.0.0.1").send().await;
    assert_eq!(r4.status(), StatusCode::OK);
}

#[tokio::test]
async fn tiered_rate_limit_integration() {
    let client = App::test()
        .use_mw(
            RateLimit::new()
                .tier("basic", 1, Duration::from_secs(60))
                .tier("premium", 1000, Duration::from_secs(60))
                .resolve_tier(|ctx: &Context| {
                    ctx.headers()
                        .get("x-tier")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("basic")
                        .to_string()
                }),
        )
        .get("/", |_: Request| "ok")
        .into_client();

    // Basic hits limit fast
    client.get("/").header("x-tier", "basic").send().await;
    let resp = client.get("/").header("x-tier", "basic").send().await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Premium has plenty of headroom
    let resp = client.get("/").header("x-tier", "premium").send().await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.header("x-ratelimit-limit"), Some("1000"));
}

#[tokio::test]
async fn cors_and_rate_limit_compose() {
    let client = App::test()
        .use_mw(Cors::permissive())
        .use_mw(
            RateLimit::new()
                .limit(1)
                .window(Duration::from_secs(60))
                .key(RateKey::Ip),
        )
        .get("/api", |_: Request| "data")
        .into_client();

    // First CORS request — OK with both CORS and rate limit headers
    let resp = client
        .get("/api")
        .header("origin", "https://app.com")
        .peer_ip("1.1.1.1")
        .send()
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.header("access-control-allow-origin"), Some("*"));
    assert_eq!(resp.header("x-ratelimit-remaining"), Some("0"));

    // Second request — 429 (rate limited)
    let resp = client
        .get("/api")
        .header("origin", "https://app.com")
        .peer_ip("1.1.1.1")
        .send()
        .await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}
