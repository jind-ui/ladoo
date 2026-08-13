//! Integration tests for health checks and pagination.

#![cfg(feature = "json")]

use ladoo::prelude::*;

// --- Health check integration ---

#[derive(Clone)]
struct FakeRedis {
    healthy: bool,
}

#[async_trait]
impl HealthCheckable for FakeRedis {
    fn name(&self) -> &str {
        "redis"
    }
    async fn check(&self) -> Result<()> {
        if self.healthy {
            Ok(())
        } else {
            Err(Error::internal("connection refused"))
        }
    }
}

#[tokio::test]
async fn health_endpoint_all_healthy() {
    let client = App::test()
        .provide_healthy(FakeRedis { healthy: true })
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["checks"]["redis"]["status"], "up");
}

#[tokio::test]
async fn health_endpoint_degraded() {
    let client = App::test()
        .provide_healthy(FakeRedis { healthy: true })
        .health("external", || async { Err(Error::internal("timeout")) })
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["status"], "degraded");
}

#[tokio::test]
async fn health_endpoint_unhealthy() {
    let client = App::test()
        .provide_healthy(FakeRedis { healthy: false })
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["status"], "unhealthy");
}

#[tokio::test]
async fn health_endpoint_custom_path() {
    let client = App::test()
        .health("ok", || async { Ok(()) })
        .health_config(HealthConfig::new().path("/ready"))
        .into_client();

    let resp = client.get("/ready").send().await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn health_endpoint_not_detailed() {
    let client = App::test()
        .health("ok", || async { Ok(()) })
        .health_config(HealthConfig::new().detailed(false))
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), 200);
    assert!(resp.body_bytes().is_empty());
}

#[tokio::test]
async fn health_endpoint_with_meta() {
    let client = App::test()
        .health("ok", || async { Ok(()) })
        .health_config(
            HealthConfig::new()
                .meta("version", "0.1.0")
                .meta_fn("build", || "test".to_string()),
        )
        .into_client();

    let resp = client.get("/health").send().await;
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["meta"]["version"], "0.1.0");
    assert_eq!(body["meta"]["build"], "test");
}

#[tokio::test]
async fn health_provide_healthy_also_provides_state() {
    let client = App::test()
        .provide_healthy(FakeRedis { healthy: true })
        .get("/redis", |redis: State<FakeRedis>| {
            if redis.healthy {
                "connected"
            } else {
                "disconnected"
            }
        })
        .into_client();

    let resp = client.get("/redis").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body_bytes(), b"connected");
}

// --- Pagination integration ---

#[tokio::test]
async fn paginate_extractor_in_handler() {
    let client = App::test()
        .get("/items", |page: Paginate| {
            let items: Vec<String> = (0..page.per_page)
                .map(|i| format!("item_{}", page.offset() + i))
                .collect();
            page.respond(items, 100)
        })
        .into_client();

    let resp = client.get("/items?page=2&per_page=5").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.content_type(), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["meta"]["page"], 2);
    assert_eq!(body["meta"]["per_page"], 5);
    assert_eq!(body["meta"]["total"], 100);
    assert_eq!(body["meta"]["total_pages"], 20);
    assert_eq!(body["data"][0], "item_5");
}

#[tokio::test]
async fn paginate_with_custom_config() {
    let client = App::test()
        .pagination(PaginationConfig::new().default_per_page(5).max_per_page(10))
        .get("/items", |page: Paginate| page.respond(vec!["a"], 1))
        .into_client();

    // Default per_page should be 5 from config
    let resp = client.get("/items").send().await;
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["meta"]["per_page"], 5);

    // per_page=50 should be clamped to 10
    let resp = client.get("/items?per_page=50").send().await;
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["meta"]["per_page"], 10);
}

#[tokio::test]
async fn cursor_extractor_in_handler() {
    let client = App::test()
        .get("/posts", |cursor: CursorParams| {
            let data = vec!["post_1", "post_2"];
            let next = if cursor.after.is_none() {
                Some("cursor_2".to_string())
            } else {
                None
            };
            cursor.respond(data, next)
        })
        .into_client();

    let resp = client.get("/posts").send().await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["meta"]["next_cursor"], "cursor_2");
    assert!(body["meta"]["has_more"].as_bool().unwrap());
}

#[tokio::test]
async fn cursor_after_and_before_returns_400() {
    let client = App::test()
        .get("/posts", |_cursor: CursorParams| "ok")
        .into_client();

    let resp = client.get("/posts?after=a&before=b").send().await;
    assert_eq!(resp.status(), 400);
}

// --- Combined health + pagination ---

#[tokio::test]
async fn health_and_pagination_coexist() {
    let client = App::test()
        .provide_healthy(FakeRedis { healthy: true })
        .pagination(PaginationConfig::new().default_per_page(10))
        .get("/items", |page: Paginate| page.respond(vec!["x"], 1))
        .into_client();

    let health = client.get("/health").send().await;
    assert_eq!(health.status(), 200);

    let items = client.get("/items").send().await;
    assert_eq!(items.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(items.body_bytes()).unwrap();
    assert_eq!(body["meta"]["per_page"], 10);
}
