//! Integration test for the public `shutdown_timeout` builder API.
//!
//! `App::into_parts()` is `pub(crate)` — it exposes the framework's
//! internal `TypeMap`, which is intentionally not part of the public API
//! (see `crates/ladoo/src/state.rs`). Integration tests live outside the
//! crate and can therefore only see `pub` items, so this test exercises
//! `shutdown_timeout()` end to end through the public `App` builder and
//! `TestClient`, rather than reaching into internals.
//!
//! Coverage of the stored duration itself (`shutdown_timeout_stores_duration`,
//! `default_shutdown_timeout_is_30s`) and of the actual signal-driven
//! drain/timeout behavior lives in `crates/ladoo/src/app.rs` and
//! `crates/ladoo/src/server.rs` unit tests, which have `pub(crate)` access.

use std::time::Duration;

use ladoo::prelude::*;

#[tokio::test]
async fn shutdown_timeout_builder_is_chainable() {
    let client = App::test()
        .shutdown_timeout(Duration::from_secs(45))
        .get("/", |_: Request| "hello")
        .into_client();

    let resp = client.get("/").send().await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text(), "hello");
}
