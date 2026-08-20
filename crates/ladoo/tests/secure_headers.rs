#![cfg(feature = "secure-headers")]

use ladoo::prelude::*;

#[tokio::test]
async fn default_secure_headers_via_test_client() {
    let client = App::test()
        .use_mw(SecureHeaders::default())
        .get("/", |_: Request| "hello")
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.header("strict-transport-security"),
        Some("max-age=63072000; includeSubDomains")
    );
    assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(resp.header("x-frame-options"), Some("DENY"));
    assert_eq!(
        resp.header("content-security-policy"),
        Some("default-src 'self'")
    );
    assert_eq!(
        resp.header("referrer-policy"),
        Some("strict-origin-when-cross-origin")
    );
    assert_eq!(
        resp.header("permissions-policy"),
        Some("camera=(), microphone=(), geolocation=()")
    );
}

#[tokio::test]
async fn handler_set_header_takes_precedence() {
    async fn override_csp(
        ctx: ladoo::context::Context,
        next: ladoo::middleware::Next,
    ) -> ladoo::error::Result<Response> {
        let mut resp = next.run(ctx).await?;
        resp.set_header("Content-Security-Policy", "script-src 'self'");
        Ok(resp)
    }

    let client = App::test()
        .use_mw(SecureHeaders::default())
        .use_mw(override_csp)
        .get("/", |_: Request| "hello")
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(
        resp.header("content-security-policy"),
        Some("script-src 'self'")
    );
    assert_eq!(resp.header("x-frame-options"), Some("DENY"));
}

#[tokio::test]
async fn customized_secure_headers() {
    let client = App::test()
        .use_mw(
            SecureHeaders::new()
                .hsts("max-age=0")
                .x_frame_options(None)
                .permissions_policy("camera=(self)"),
        )
        .get("/", |_: Request| "hello")
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(
        resp.header("strict-transport-security"),
        Some("max-age=0")
    );
    assert!(resp.header("x-frame-options").is_none());
    assert_eq!(
        resp.header("permissions-policy"),
        Some("camera=(self)")
    );
    assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
}
