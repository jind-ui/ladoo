//! Health check endpoint with auto-discovery.
//!
//! Register health-checkable types with
//! [`App::provide_healthy()`](crate::app::App::provide_healthy) or
//! manual closures with [`App::health()`](crate::app::App::health).
//! The framework auto-registers a `GET /health` endpoint that runs all
//! checks concurrently and returns structured JSON.
//!
//! # Status codes
//!
//! | Condition | Code | Body `status` |
//! |-----------|------|---------------|
//! | All pass  | 200  | `"healthy"`   |
//! | Some fail | 200  | `"degraded"`  |
//! | All fail  | 503  | `"unhealthy"` |
//! | None registered | 200 | `"healthy"` |

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::response::Response;
use crate::state::State;

/// A type that can report its health status.
///
/// Implement this on database pools, cache clients, or any dependency
/// whose availability matters. Register with
/// [`App::provide_healthy()`](crate::app::App::provide_healthy) to
/// auto-include in the health endpoint.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// struct Database { /* ... */ }
///
/// #[async_trait]
/// impl HealthCheckable for Database {
///     fn name(&self) -> &str { "postgres" }
///     async fn check(&self) -> Result<()> {
///         self.ping().await.map_err(|e| Error::internal(e.to_string()))
///     }
/// }
/// ```
#[async_trait]
pub trait HealthCheckable: Send + Sync + 'static {
    /// Display name for this check (e.g., "postgres", "redis").
    fn name(&self) -> &str;

    /// Return `Ok(())` if healthy, `Err` with reason if not.
    async fn check(&self) -> crate::error::Result<()>;
}

#[derive(Debug)]
pub(crate) struct CheckResult {
    name: String,
    ok: bool,
    error: Option<String>,
    latency: Duration,
}

/// A boxed closure used for manual health checks registered via
/// [`App::health()`](crate::app::App::health).
pub(crate) type HealthCheckFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = crate::error::Result<()>> + Send>> + Send + Sync>;

pub(crate) struct HealthClosure {
    pub(crate) name: String,
    pub(crate) check: HealthCheckFn,
}

/// Internal registry of health checks.
///
/// Populated by [`App::provide_healthy()`](crate::app::App::provide_healthy)
/// and [`App::health()`](crate::app::App::health). Stored in application
/// state (as `Arc<HealthRegistry>`) and read by the health handler.
#[derive(Default)]
pub(crate) struct HealthRegistry {
    pub(crate) checks: Vec<Arc<dyn HealthCheckable>>,
    pub(crate) closures: Vec<HealthClosure>,
}

impl HealthRegistry {
    pub(crate) fn new() -> Self {
        Self {
            checks: Vec::new(),
            closures: Vec::new(),
        }
    }

    pub(crate) fn has_checks(&self) -> bool {
        !self.checks.is_empty() || !self.closures.is_empty()
    }

    pub(crate) async fn run_checks(&self, timeout: Duration) -> Vec<CheckResult> {
        let mut set = tokio::task::JoinSet::new();
        let mut names: HashMap<tokio::task::Id, String> = HashMap::new();

        for check in &self.checks {
            let check = check.clone();
            let name = check.name().to_string();
            let fut = async move { check.check().await };
            let abort = set.spawn(run_one(name.clone(), fut, timeout));
            names.insert(abort.id(), name);
        }

        for closure in &self.closures {
            let name = closure.name.clone();
            let check_fn = closure.check.clone();
            let fut = async move { check_fn().await };
            let abort = set.spawn(run_one(name.clone(), fut, timeout));
            names.insert(abort.id(), name);
        }

        let mut results = Vec::new();
        while let Some(joined) = set.join_next_with_id().await {
            match joined {
                Ok((_, result)) => results.push(result),
                Err(join_err) => {
                    let name = names
                        .get(&join_err.id())
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    results.push(CheckResult {
                        name,
                        ok: false,
                        error: Some("check panicked".to_string()),
                        latency: Duration::ZERO,
                    });
                }
            }
        }
        results
    }
}

/// Run a single health check with a timeout, producing a [`CheckResult`]
/// regardless of whether the check succeeds, fails, or times out.
///
/// Shared by both [`HealthCheckable`] checks and manual [`HealthClosure`]
/// checks in [`HealthRegistry::run_checks`] — the only difference between
/// the two is how the name and future are obtained.
async fn run_one(
    name: String,
    fut: impl Future<Output = crate::error::Result<()>> + Send,
    timeout: Duration,
) -> CheckResult {
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(timeout, fut).await;
    let latency = start.elapsed();
    CheckResult {
        name,
        ok: matches!(result, Ok(Ok(()))),
        error: match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some("check timed out".to_string()),
        },
        latency,
    }
}

/// Configuration for the health endpoint.
///
/// Controls the endpoint path, response format, metadata, and per-check
/// timeout. Register with
/// [`App::health_config()`](crate::app::App::health_config).
///
/// # Examples
///
/// ```rust
/// use ladoo::health::HealthConfig;
/// use std::time::Duration;
///
/// let config = HealthConfig::new()
///     .path("/healthz")
///     .detailed(false)
///     .include_latency(true)
///     .meta("version", env!("CARGO_PKG_VERSION"))
///     .check_timeout(Duration::from_secs(10));
/// ```
pub struct HealthConfig {
    /// URL path for the health endpoint.
    pub(crate) path: String,
    /// Show per-check status in the response body.
    pub(crate) detailed: bool,
    /// Include check latency in the response.
    pub(crate) include_latency: bool,
    /// Static metadata key-value pairs.
    pub(crate) meta: HashMap<String, String>,
    /// Dynamic metadata closures evaluated on each request.
    pub(crate) meta_fns: Vec<(String, Arc<dyn Fn() -> String + Send + Sync>)>,
    /// Timeout for each individual health check.
    pub(crate) check_timeout: Duration,
}

impl HealthConfig {
    /// Create a new config with defaults.
    ///
    /// Defaults: path `/health`, detailed `true`, no latency, 5s timeout.
    pub fn new() -> Self {
        Self {
            path: "/health".to_string(),
            detailed: true,
            include_latency: false,
            meta: HashMap::new(),
            meta_fns: Vec::new(),
            check_timeout: Duration::from_secs(5),
        }
    }

    /// Set the endpoint path (default: `/health`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Show structured per-check status in the response (default: `true`).
    ///
    /// Set to `false` in production to return only a status code with no body.
    pub fn detailed(mut self, detailed: bool) -> Self {
        self.detailed = detailed;
        self
    }

    /// Include per-check latency in milliseconds (default: `false`).
    pub fn include_latency(mut self, include: bool) -> Self {
        self.include_latency = include;
        self
    }

    /// Add a static metadata key-value pair to the response.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.meta.insert(key.into(), value.into());
        self
    }

    /// Add a dynamic metadata entry evaluated on each health check request.
    pub fn meta_fn(
        mut self,
        key: impl Into<String>,
        f: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        self.meta_fns.push((key.into(), Arc::new(f)));
        self
    }

    /// Set the per-check timeout (default: 5 seconds).
    pub fn check_timeout(mut self, timeout: Duration) -> Self {
        self.check_timeout = timeout;
        self
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The auto-registered health endpoint handler.
///
/// Runs all registered checks concurrently, determines overall status,
/// and returns a JSON response (or empty body if `detailed` is false).
pub(crate) async fn health_handler(
    registry: State<HealthRegistry>,
    config: State<HealthConfig>,
) -> Response {
    let results = registry.run_checks(config.check_timeout).await;

    let (status_str, status_code) = if results.is_empty() {
        ("healthy", http::StatusCode::OK)
    } else {
        let pass_count = results.iter().filter(|r| r.ok).count();
        if pass_count == results.len() {
            ("healthy", http::StatusCode::OK)
        } else if pass_count > 0 {
            ("degraded", http::StatusCode::OK)
        } else {
            ("unhealthy", http::StatusCode::SERVICE_UNAVAILABLE)
        }
    };

    if !config.detailed {
        return Response::empty(status_code);
    }

    let mut body = serde_json::Map::new();
    body.insert(
        "status".into(),
        serde_json::Value::String(status_str.into()),
    );

    if !results.is_empty() {
        let mut checks = serde_json::Map::new();
        for result in &results {
            let mut check = serde_json::Map::new();
            check.insert(
                "status".into(),
                serde_json::Value::String(if result.ok { "up" } else { "down" }.into()),
            );
            if let Some(err) = &result.error {
                if crate::error::is_dev_mode() {
                    check.insert("error".into(), serde_json::Value::String(err.clone()));
                } else {
                    #[cfg(feature = "logging")]
                    tracing::warn!(
                        check = %result.name,
                        error = %err,
                        "health check failed"
                    );
                    check.insert(
                        "error".into(),
                        serde_json::Value::String("check failed".into()),
                    );
                }
            }
            if config.include_latency {
                check.insert(
                    "latency_ms".into(),
                    serde_json::Value::Number(serde_json::Number::from(
                        result.latency.as_millis() as u64
                    )),
                );
            }
            checks.insert(result.name.clone(), serde_json::Value::Object(check));
        }
        body.insert("checks".into(), serde_json::Value::Object(checks));
    }

    let has_meta = !config.meta.is_empty() || !config.meta_fns.is_empty();
    if has_meta {
        let mut meta = serde_json::Map::new();
        for (k, v) in &config.meta {
            meta.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        for (k, f) in &config.meta_fns {
            meta.insert(k.clone(), serde_json::Value::String(f()));
        }
        body.insert("meta".into(), serde_json::Value::Object(meta));
    }

    let json = serde_json::to_string(&body).expect("health JSON serialization cannot fail");
    Response::with_json_body(status_code, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysHealthy;

    #[async_trait]
    impl HealthCheckable for AlwaysHealthy {
        fn name(&self) -> &str {
            "always-healthy"
        }
        async fn check(&self) -> crate::error::Result<()> {
            Ok(())
        }
    }

    struct AlwaysSick;

    #[async_trait]
    impl HealthCheckable for AlwaysSick {
        fn name(&self) -> &str {
            "always-sick"
        }
        async fn check(&self) -> crate::error::Result<()> {
            Err(crate::error::Error::internal("connection refused"))
        }
    }

    #[tokio::test]
    async fn healthy_check_returns_ok() {
        let check = AlwaysHealthy;
        assert!(check.check().await.is_ok());
    }

    #[tokio::test]
    async fn unhealthy_check_returns_err() {
        let check = AlwaysSick;
        assert!(check.check().await.is_err());
    }

    #[tokio::test]
    async fn registry_all_healthy() {
        let mut registry = HealthRegistry::new();
        registry.checks.push(Arc::new(AlwaysHealthy));
        let results = registry.run_checks(Duration::from_secs(5)).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert!(results[0].error.is_none());
    }

    #[tokio::test]
    async fn registry_one_unhealthy() {
        let mut registry = HealthRegistry::new();
        registry.checks.push(Arc::new(AlwaysHealthy));
        registry.checks.push(Arc::new(AlwaysSick));
        let results = registry.run_checks(Duration::from_secs(5)).await;
        assert_eq!(results.len(), 2);
        let healthy_count = results.iter().filter(|r| r.ok).count();
        let sick_count = results.iter().filter(|r| !r.ok).count();
        assert_eq!(healthy_count, 1);
        assert_eq!(sick_count, 1);
    }

    #[tokio::test]
    async fn registry_check_timeout() {
        struct SlowCheck;
        #[async_trait]
        impl HealthCheckable for SlowCheck {
            fn name(&self) -> &str {
                "slow"
            }
            async fn check(&self) -> crate::error::Result<()> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(())
            }
        }
        let mut registry = HealthRegistry::new();
        registry.checks.push(Arc::new(SlowCheck));
        let results = registry.run_checks(Duration::from_millis(50)).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert_eq!(results[0].error.as_deref(), Some("check timed out"));
    }

    #[tokio::test]
    async fn registry_panicking_check_reports_others_correctly() {
        struct PanickingCheck;
        #[async_trait]
        impl HealthCheckable for PanickingCheck {
            fn name(&self) -> &str {
                "panicky"
            }
            async fn check(&self) -> crate::error::Result<()> {
                panic!("boom");
            }
        }
        let mut registry = HealthRegistry::new();
        registry.checks.push(Arc::new(AlwaysHealthy));
        registry.checks.push(Arc::new(PanickingCheck));
        let results = registry.run_checks(Duration::from_secs(5)).await;

        // Both results must be present — a panic in one check must not
        // silently truncate the others.
        assert_eq!(results.len(), 2);

        let healthy = results
            .iter()
            .find(|r| r.name == "always-healthy")
            .expect("healthy check result missing");
        assert!(healthy.ok);
        assert!(healthy.error.is_none());

        let panicked = results
            .iter()
            .find(|r| r.name == "panicky")
            .expect("panicked check result missing");
        assert!(!panicked.ok);
        assert_eq!(panicked.error.as_deref(), Some("check panicked"));
    }

    #[tokio::test]
    async fn registry_closure_check() {
        let mut registry = HealthRegistry::new();
        registry.closures.push(HealthClosure {
            name: "closure-check".into(),
            check: Arc::new(|| Box::pin(async { Ok(()) })),
        });
        let results = registry.run_checks(Duration::from_secs(5)).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert_eq!(results[0].name, "closure-check");
    }

    #[tokio::test]
    async fn registry_empty_returns_empty() {
        let registry = HealthRegistry::new();
        let results = registry.run_checks(Duration::from_secs(5)).await;
        assert!(results.is_empty());
    }

    #[test]
    fn health_config_defaults() {
        let config = HealthConfig::new();
        assert_eq!(config.path, "/health");
        assert!(config.detailed);
        assert!(!config.include_latency);
        assert!(config.meta.is_empty());
        assert!(config.meta_fns.is_empty());
        assert_eq!(config.check_timeout, Duration::from_secs(5));
    }

    #[test]
    fn health_config_builder() {
        let config = HealthConfig::new()
            .path("/healthz")
            .detailed(false)
            .include_latency(true)
            .check_timeout(Duration::from_secs(10))
            .meta("version", "1.0");
        assert_eq!(config.path, "/healthz");
        assert!(!config.detailed);
        assert!(config.include_latency);
        assert_eq!(config.check_timeout, Duration::from_secs(10));
        assert_eq!(config.meta.get("version"), Some(&"1.0".to_string()));
    }

    #[test]
    fn health_config_meta_fn() {
        let config = HealthConfig::new().meta_fn("counter", || "42".to_string());
        assert_eq!(config.meta_fns.len(), 1);
        assert_eq!(config.meta_fns[0].0, "counter");
        assert_eq!((config.meta_fns[0].1)(), "42");
    }

    fn make_registry(checks: Vec<Arc<dyn HealthCheckable>>) -> HealthRegistry {
        HealthRegistry {
            checks,
            closures: Vec::new(),
        }
    }

    #[tokio::test]
    async fn handler_all_healthy() {
        let registry = make_registry(vec![Arc::new(AlwaysHealthy)]);
        let config = HealthConfig::new();
        let resp = health_handler(State::new(registry), State::new(config)).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["status"], "healthy");
    }

    #[tokio::test]
    async fn handler_degraded() {
        let registry = make_registry(vec![Arc::new(AlwaysHealthy), Arc::new(AlwaysSick)]);
        let config = HealthConfig::new();
        let resp = health_handler(State::new(registry), State::new(config)).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["status"], "degraded");
    }

    #[tokio::test]
    async fn handler_all_unhealthy() {
        let registry = make_registry(vec![Arc::new(AlwaysSick)]);
        let config = HealthConfig::new();
        let resp = health_handler(State::new(registry), State::new(config)).await;
        assert_eq!(resp.status(), http::StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["status"], "unhealthy");
    }

    #[tokio::test]
    async fn handler_no_checks_is_healthy() {
        let registry = make_registry(vec![]);
        let config = HealthConfig::new();
        let resp = health_handler(State::new(registry), State::new(config)).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["status"], "healthy");
    }

    #[tokio::test]
    async fn handler_not_detailed_returns_empty_body() {
        let registry = make_registry(vec![Arc::new(AlwaysHealthy)]);
        let config = HealthConfig::new().detailed(false);
        let resp = health_handler(State::new(registry), State::new(config)).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert!(resp.body_bytes().is_empty());
    }

    #[tokio::test]
    async fn handler_includes_latency() {
        let registry = make_registry(vec![Arc::new(AlwaysHealthy)]);
        let config = HealthConfig::new().include_latency(true);
        let resp = health_handler(State::new(registry), State::new(config)).await;
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        let check = &body["checks"]["always-healthy"];
        assert!(check["latency_ms"].is_number());
    }

    #[tokio::test]
    async fn handler_includes_meta() {
        let registry = make_registry(vec![]);
        let config = HealthConfig::new()
            .meta("version", "1.0")
            .meta_fn("dynamic", || "computed".into());
        let resp = health_handler(State::new(registry), State::new(config)).await;
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["meta"]["version"], "1.0");
        assert_eq!(body["meta"]["dynamic"], "computed");
    }

    // These three tests deliberately hold the env-var lock across an `.await`
    // point: the point is to serialize the *entire* env-var-dependent
    // request/response cycle against other tests in the process that touch
    // `LADOO_ENV`/`APP_ENV`, not just the synchronous `set_var`/`remove_var`
    // calls. The lock is a plain `std::sync::Mutex` (see
    // `crate::error::tests::lock_env`) rather than an async-aware one because
    // it only ever guards a few microseconds of test setup/teardown, never
    // real request handling.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn handler_failed_check_shows_error() {
        let _guard = crate::error::tests::lock_env();
        std::env::set_var("LADOO_ENV", "development");
        let registry = make_registry(vec![Arc::new(AlwaysSick)]);
        let config = HealthConfig::new();
        let resp = health_handler(State::new(registry), State::new(config)).await;
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        let check = &body["checks"]["always-sick"];
        assert_eq!(check["status"], "down");
        assert!(check["error"]
            .as_str()
            .unwrap()
            .contains("connection refused"));
        std::env::remove_var("LADOO_ENV");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn health_redacts_errors_in_prod_mode() {
        let _guard = crate::error::tests::lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");
        // No env var = production mode (after Task 1's default flip)

        let app = crate::app::App::new()
            .health("failing", || async {
                Err(crate::error::Error::internal(
                    "connection refused: postgres://admin:secret@db:5432",
                ))
            })
            .health_config(HealthConfig::default());

        let client = app.into_client();
        let resp = client.get("/health").send().await;
        let body = resp.text();

        // Must contain "check failed", NOT the actual error with connection string
        assert!(
            body.contains("check failed"),
            "Error should be redacted in prod"
        );
        assert!(
            !body.contains("postgres://"),
            "Connection string must not leak in prod"
        );
        assert!(
            !body.contains("connection refused"),
            "Error details must not leak in prod"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn health_shows_full_error_in_dev_mode() {
        let _guard = crate::error::tests::lock_env();
        std::env::set_var("LADOO_ENV", "development");

        let app = crate::app::App::new()
            .health("failing", || async {
                Err(crate::error::Error::internal(
                    "connection refused: postgres://db:5432",
                ))
            })
            .health_config(HealthConfig::default());

        let client = app.into_client();
        let resp = client.get("/health").send().await;
        let body = resp.text();

        assert!(
            body.contains("connection refused"),
            "Dev mode should show full error"
        );

        std::env::remove_var("LADOO_ENV");
    }
}
