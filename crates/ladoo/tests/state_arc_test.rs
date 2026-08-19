//! Integration tests for the Phase 22 `State<T>` Arc-based storage.
//!
//! These exercise the full stack — `App::provide` through routing,
//! middleware, and the `State<T>` extractor — proving that non-`Clone`
//! types can be provided and that state is shared (not copied) across
//! requests.

use std::sync::atomic::{AtomicU64, Ordering};

use ladoo::prelude::*;

#[tokio::test]
async fn state_extraction_with_non_clone_type() {
    struct UniqueResource {
        id: u64,
    }

    let client = App::test()
        .provide(UniqueResource { id: 42 })
        .get("/resource", |res: State<UniqueResource>| {
            format!("id={}", res.id)
        })
        .into_client();

    let resp = client.get("/resource").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "id=42");
}

#[tokio::test]
async fn state_arc_identity_across_requests() {
    struct Counter(AtomicU64);

    let client = App::test()
        .provide(Counter(AtomicU64::new(0)))
        .get("/inc", |counter: State<Counter>| {
            let val = counter.0.fetch_add(1, Ordering::SeqCst);
            format!("count={val}")
        })
        .into_client();

    let r1 = client.get("/inc").send().await;
    assert_eq!(r1.text(), "count=0");
    let r2 = client.get("/inc").send().await;
    assert_eq!(r2.text(), "count=1");
}
