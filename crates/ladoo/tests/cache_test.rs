//! Integration tests for the caching subsystem.

use std::time::Duration;

use ladoo::cache::MemoryStore;
use ladoo::prelude::*;

#[tokio::test]
async fn cache_via_state_get_and_set() {
    let client = App::test()
        .provide(Cache::new(MemoryStore::new()))
        .get("/set", |cache: State<Cache>| async move {
            cache.set("greeting", &"hello", None).await.unwrap();
            "stored"
        })
        .get("/get", |cache: State<Cache>| async move {
            let val: Option<String> = cache.get("greeting").await.unwrap();
            val.unwrap_or_else(|| "empty".into())
        })
        .into_client();

    let resp = client.get("/set").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "stored");

    let resp = client.get("/get").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "hello");
}

#[tokio::test]
async fn cache_remember_returns_computed_value() {
    let client = App::test()
        .provide(Cache::new(MemoryStore::new()))
        .get("/compute", |cache: State<Cache>| async move {
            let val: u64 = cache
                .remember("answer", None, || async { Ok(42_u64) })
                .await
                .unwrap();
            val.to_string()
        })
        .into_client();

    let resp = client.get("/compute").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "42");
}

#[tokio::test]
async fn cache_with_default_ttl_expires() {
    let client = App::test()
        .provide(Cache::new(MemoryStore::new()).default_ttl(Duration::from_millis(1)))
        .get("/set", |cache: State<Cache>| async move {
            cache.set("temp", &"ephemeral", None).await.unwrap();
            "ok"
        })
        .get("/get", |cache: State<Cache>| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let val: Option<String> = cache.get("temp").await.unwrap();
            match val {
                Some(v) => v,
                None => "expired".into(),
            }
        })
        .into_client();

    client.get("/set").send().await;
    let resp = client.get("/get").send().await;
    assert_eq!(resp.text(), "expired");
}

#[tokio::test]
async fn cache_delete_invalidates_entry() {
    let client = App::test()
        .provide(Cache::new(MemoryStore::new()))
        .get("/setup", |cache: State<Cache>| async move {
            cache.set("key", &"value", None).await.unwrap();
            "ok"
        })
        .get("/delete", |cache: State<Cache>| async move {
            let existed = cache.delete("key").await.unwrap();
            existed.to_string()
        })
        .get("/check", |cache: State<Cache>| async move {
            let exists = cache.has("key").await.unwrap();
            exists.to_string()
        })
        .into_client();

    client.get("/setup").send().await;
    let resp = client.get("/delete").send().await;
    assert_eq!(resp.text(), "true");
    let resp = client.get("/check").send().await;
    assert_eq!(resp.text(), "false");
}

#[tokio::test]
async fn per_api_caches_with_newtypes() {
    #[derive(Clone)]
    struct UserCache(Cache);
    #[derive(Clone)]
    struct ConfigCache(Cache);

    let client = App::test()
        .provide(UserCache(
            Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(60)),
        ))
        .provide(ConfigCache(
            Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(3600)),
        ))
        .get("/user", |cache: State<UserCache>| async move {
            cache.0 .0.set("user:1", &"Alice", None).await.unwrap();
            "ok"
        })
        .get("/config", |cache: State<ConfigCache>| async move {
            let user_val: Option<String> = cache.0 .0.get("user:1").await.unwrap();
            match user_val {
                Some(_) => "leaked",
                None => "isolated",
            }
        })
        .into_client();

    client.get("/user").send().await;
    let resp = client.get("/config").send().await;
    assert_eq!(resp.text(), "isolated");
}

#[tokio::test]
async fn cache_missing_state_returns_500() {
    let client = App::test()
        .get("/no-cache", |cache: State<Cache>| async move {
            let _: Option<String> = cache.get("key").await.unwrap();
            "ok"
        })
        .into_client();

    let resp = client.get("/no-cache").send().await;
    assert_eq!(resp.status(), 500);
}
