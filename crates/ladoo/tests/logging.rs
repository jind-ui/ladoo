#![cfg(feature = "logging")]

use ladoo::prelude::*;

/// Test that RequestId can be extracted in a handler via State<RequestId>
#[tokio::test]
async fn request_id_extractable_in_handler() {
    let client = App::test()
        .use_mw(|mut ctx: Context, next: Next| {
            let id = ladoo::logging::RequestId("test-id-123".to_string());
            ctx.provide(id);
            async move { next.run(ctx).await }
        })
        .get("/", |id: State<RequestId>| format!("id: {}", id.0))
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "id: test-id-123");
}

/// Test that RequestId is available via the prelude re-export
#[test]
fn request_id_available_via_prelude() {
    let id = RequestId("hello".to_string());
    assert_eq!(id.to_string(), "hello");
}

/// Test that builder methods for logging are chainable
#[test]
fn builder_methods_chainable() {
    let _app = App::new()
        .log_level("debug")
        .log_filter("my_app=trace")
        .disable_request_logging()
        .request_id_header("x-trace-id")
        .get("/", |_: Request| "hello");
}
