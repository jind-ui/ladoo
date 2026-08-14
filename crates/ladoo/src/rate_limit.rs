//! Rate limiting middleware with pluggable storage.
//!
//! The [`RateLimit`] middleware tracks request counts per key (IP, header,
//! custom function) and rejects excess requests with `429 Too Many Requests`.
//! Storage is pluggable via the [`RateStore`] trait — implement it for
//! Redis, a database, or any shared backend.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//! use std::time::Duration;
//!
//! App::new()
//!     .use_mw(
//!         RateLimit::new()
//!             .limit(100)
//!             .window(Duration::from_secs(900))
//!             .key(RateKey::Ip)
//!     )
//!     .get("/api/data", handler);
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;

use crate::context::Context;
use crate::error::Result;
use crate::middleware::{Middleware, Next};
use crate::response::Response;

/// The result of a rate limit check.
///
/// Contains the limit, remaining count, and when the window resets.
/// Used by `RateLimit` to set response headers and decide whether
/// to allow the request.
pub struct RateResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// The configured limit for this window.
    pub limit: u64,
    /// Remaining requests in the current window.
    pub remaining: u64,
    /// When the current window resets.
    pub reset_at: std::time::Instant,
}

/// Pluggable storage backend for rate limit counters.
///
/// Implement this trait to back rate limiting with Redis, a database,
/// or any shared store. Ladoo ships [`MemoryStore`] for single-process
/// apps.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::rate_limit::{RateStore, RateResult};
/// use async_trait::async_trait;
/// use std::time::{Duration, Instant};
///
/// struct MyRedisStore { /* ... */ }
///
/// #[async_trait]
/// impl RateStore for MyRedisStore {
///     async fn check_and_increment(
///         &self, key: &str, limit: u64, window: Duration,
///     ) -> RateResult {
///         // query Redis INCR + EXPIRE ...
///         # todo!()
///     }
///     async fn reset(&self, key: &str) {
///         // DEL key ...
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait RateStore: Send + Sync + 'static {
    /// Check if `key` is within its limit and increment the counter.
    ///
    /// If the key has no entry or its window has expired, a new window
    /// starts. Returns a [`RateResult`] indicating whether the request
    /// is allowed and the current counter state.
    async fn check_and_increment(
        &self,
        key: &str,
        limit: u64,
        window: Duration,
    ) -> RateResult;

    /// Reset the counter for `key`.
    ///
    /// Useful after events like a successful login where you want to
    /// clear the rate limit for that client.
    async fn reset(&self, key: &str);
}

/// In-memory rate-limit store backed by [`DashMap`].
///
/// Expired entries are automatically evicted by a background task that sweeps
/// every 60 seconds (configurable via [`with_reap_interval`](Self::with_reap_interval)).
/// The reaper exits automatically when the `MemoryStore` is dropped.
///
/// For distributed deployments, implement [`RateStore`] for a shared
/// backend like Redis.
pub struct MemoryStore {
    entries: Arc<DashMap<String, (u64, tokio::time::Instant)>>,
}

impl MemoryStore {
    /// Create a new rate-limit store with a background reaper
    /// that sweeps expired entries every 60 seconds.
    pub fn new() -> Self {
        Self::with_reap_interval(Duration::from_secs(60))
    }

    /// Create a new rate-limit store with a custom reap interval.
    ///
    /// The reaper runs as a background Tokio task and removes entries whose
    /// expiry instant has passed. It exits automatically when the `MemoryStore`
    /// is dropped.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::prelude::*;
    /// use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let store = MemoryStore::with_reap_interval(Duration::from_secs(30));
    /// # let _ = store;
    /// # }
    /// ```
    pub fn with_reap_interval(interval: Duration) -> Self {
        let entries = Arc::new(DashMap::new());
        Self::spawn_reaper(Arc::downgrade(&entries), interval);
        Self { entries }
    }

    fn spawn_reaper(weak: Weak<DashMap<String, (u64, tokio::time::Instant)>>, interval: Duration) {
        // Capture the first deadline synchronously, at spawn time, rather than
        // letting the first `sleep` call compute `now + interval` whenever the
        // task happens to get its first poll. Under `tokio::time::pause`, the
        // task may not be polled until after time has already been advanced,
        // which would otherwise push the deadline further into the future than
        // intended and make the reaper miss the eviction window entirely.
        let mut deadline = tokio::time::Instant::now() + interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep_until(deadline).await;
                match weak.upgrade() {
                    Some(entries) => {
                        let now = tokio::time::Instant::now();
                        entries.retain(|_, (_, expires)| now < *expires);
                        deadline = now + interval;
                    }
                    None => break,
                }
            }
        });
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RateStore for MemoryStore {
    async fn check_and_increment(
        &self,
        key: &str,
        limit: u64,
        window: Duration,
    ) -> RateResult {
        let now = tokio::time::Instant::now();

        let mut entry = self.entries.entry(key.to_string()).or_insert_with(|| {
            (0, now + window)
        });

        let (count, expires) = entry.value_mut();

        // Window expired — reset
        if now >= *expires {
            *count = 0;
            *expires = now + window;
        }

        *count += 1;
        let current_count = *count;
        let reset_at = (*expires).into_std();

        if current_count <= limit {
            RateResult {
                allowed: true,
                limit,
                remaining: limit - current_count,
                reset_at,
            }
        } else {
            RateResult {
                allowed: false,
                limit,
                remaining: 0,
                reset_at,
            }
        }
    }

    async fn reset(&self, key: &str) {
        self.entries.remove(key);
    }
}

/// How to identify a client for rate limiting.
///
/// Each variant extracts a string key from the request. Requests with
/// the same key share a counter.
pub enum RateKey {
    /// Client IP address from the TCP socket.
    Ip,
    /// Value of a specific request header.
    Header(String),
    /// Custom key function.
    Custom(TierResolver),
}

/// A function that maps a request [`Context`] to a `String`.
///
/// Used both for [`RateKey::Custom`] key extraction and for
/// [`RateLimit::resolve_tier`] tier resolution.
type TierResolver = Arc<dyn Fn(&Context) -> String + Send + Sync>;

enum RateLimitConfig {
    Simple {
        limit: u64,
        window: Duration,
        key: RateKey,
    },
    Tiered {
        tiers: Vec<Tier>,
        resolve: Option<TierResolver>,
    },
}

struct Tier {
    name: String,
    limit: u64,
    window: Duration,
}

/// Rate limiting middleware with pluggable storage.
///
/// Tracks request counts per key and rejects excess requests with
/// `429 Too Many Requests`. Supports simple limits (one limit for
/// all requests) and tiered limits (different limits per user plan).
///
/// # Simple Mode
///
/// ```rust,ignore
/// use ladoo::prelude::*;
/// use std::time::Duration;
///
/// App::new()
///     .use_mw(
///         RateLimit::new()
///             .limit(100)
///             .window(Duration::from_secs(900))
///             .key(RateKey::Ip)
///     )
///     .get("/api", handler);
/// ```
///
/// # Tiered Mode
///
/// ```rust,ignore
/// App::new()
///     .use_mw(
///         RateLimit::new()
///             .tier("free", 100, Duration::from_secs(3600))
///             .tier("pro", 10_000, Duration::from_secs(3600))
///             .resolve_tier(|ctx: &Context| {
///                 // Extract the plan from auth state
///                 "free".to_string()
///             })
///     )
///     .get("/api", handler);
/// ```
pub struct RateLimit<S: RateStore = MemoryStore> {
    config: RateLimitConfig,
    store: Arc<S>,
}

impl RateLimit<MemoryStore> {
    /// Create a rate limiter with default settings.
    ///
    /// Defaults: 100 requests per 15 minutes, keyed by IP, using
    /// in-memory storage.
    pub fn new() -> Self {
        Self {
            config: RateLimitConfig::Simple {
                limit: 100,
                window: Duration::from_secs(900),
                key: RateKey::Ip,
            },
            store: Arc::new(MemoryStore::new()),
        }
    }
}

impl Default for RateLimit<MemoryStore> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: RateStore> RateLimit<S> {
    /// Set the maximum number of requests per window.
    ///
    /// Ignored when tiered mode is active (use [`tier`](Self::tier)
    /// instead).
    pub fn limit(mut self, limit: u64) -> Self {
        if let RateLimitConfig::Simple {
            limit: ref mut l, ..
        } = self.config
        {
            *l = limit;
        }
        self
    }

    /// Set the time window for the rate limit.
    ///
    /// Ignored when tiered mode is active.
    pub fn window(mut self, window: Duration) -> Self {
        if let RateLimitConfig::Simple {
            window: ref mut w, ..
        } = self.config
        {
            *w = window;
        }
        self
    }

    /// Set how to identify clients.
    ///
    /// Ignored when tiered mode is active (tiered mode always uses
    /// the resolve function output as the key prefix).
    pub fn key(mut self, key: RateKey) -> Self {
        if let RateLimitConfig::Simple {
            key: ref mut k, ..
        } = self.config
        {
            *k = key;
        }
        self
    }

    /// Use a different storage backend.
    ///
    /// Replaces the default [`MemoryStore`] with a custom
    /// [`RateStore`] implementation.
    pub fn store<S2: RateStore>(self, store: S2) -> RateLimit<S2> {
        RateLimit {
            config: self.config,
            store: Arc::new(store),
        }
    }

    /// Add a rate limit tier.
    ///
    /// The first call to `tier` switches from simple mode to tiered
    /// mode. Each tier has a name, a request limit, and a time window.
    /// Use [`resolve_tier`](Self::resolve_tier) to map each request
    /// to a tier name.
    pub fn tier(mut self, name: &str, limit: u64, window: Duration) -> Self {
        match &mut self.config {
            RateLimitConfig::Simple { .. } => {
                self.config = RateLimitConfig::Tiered {
                    tiers: vec![Tier {
                        name: name.to_string(),
                        limit,
                        window,
                    }],
                    resolve: None,
                };
            }
            RateLimitConfig::Tiered { tiers, .. } => {
                tiers.push(Tier {
                    name: name.to_string(),
                    limit,
                    window,
                });
            }
        }
        self
    }

    /// Set the function that maps a request to a tier name.
    ///
    /// The closure receives the [`Context`] (after upstream middleware
    /// has run) and returns the tier name as a `String`. If the
    /// returned name doesn't match any defined tier, the first tier
    /// is used as a fallback.
    pub fn resolve_tier<F>(mut self, f: F) -> Self
    where
        F: Fn(&Context) -> String + Send + Sync + 'static,
    {
        if let RateLimitConfig::Tiered { resolve, .. } = &mut self.config {
            *resolve = Some(Arc::new(f));
        }
        self
    }

    fn extract_key(&self, ctx: &Context) -> String {
        match &self.config {
            RateLimitConfig::Simple { key, .. } => match key {
                RateKey::Ip => ctx.peer_ip().unwrap_or("unknown").to_string(),
                RateKey::Header(name) => ctx
                    .headers()
                    .get(name.as_str())
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown")
                    .to_string(),
                RateKey::Custom(f) => f(ctx),
            },
            RateLimitConfig::Tiered { resolve, .. } => {
                let tier_name = resolve
                    .as_ref()
                    .map(|f| f(ctx))
                    .unwrap_or_else(|| "default".to_string());
                // Include the resolved tier as the key prefix so
                // different tiers sharing the same underlying identity
                // don't collide.
                format!("tier:{tier_name}")
            }
        }
    }

    fn resolve_limit_and_window(&self, ctx: &Context) -> (u64, Duration) {
        match &self.config {
            RateLimitConfig::Simple { limit, window, .. } => (*limit, *window),
            RateLimitConfig::Tiered { tiers, resolve, .. } => {
                let tier_name = resolve.as_ref().map(|f| f(ctx)).unwrap_or_default();
                tiers
                    .iter()
                    .find(|t| t.name == tier_name)
                    .or_else(|| tiers.first())
                    .map(|t| (t.limit, t.window))
                    .unwrap_or((100, Duration::from_secs(900)))
            }
        }
    }
}

impl<S: RateStore + 'static> Middleware for RateLimit<S> {
    fn call(
        &self,
        ctx: Context,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
        let store = self.store.clone();
        let key = self.extract_key(&ctx);
        let (limit, window) = self.resolve_limit_and_window(&ctx);

        Box::pin(async move {
            let result = store.check_and_increment(&key, limit, window).await;

            let reset_secs = result
                .reset_at
                .saturating_duration_since(std::time::Instant::now())
                .as_secs();

            if !result.allowed {
                let body = format!(
                    "{{\"error\":\"rate limit exceeded\",\"retry_after\":{}}}",
                    reset_secs
                );
                let mut resp =
                    Response::with_json_body(http::StatusCode::TOO_MANY_REQUESTS, &body);
                resp.set_header("x-ratelimit-limit", &result.limit.to_string());
                resp.set_header("x-ratelimit-remaining", "0");
                resp.set_header("x-ratelimit-reset", &reset_secs.to_string());
                resp.set_header("retry-after", &reset_secs.to_string());
                return Ok(resp);
            }

            let mut resp = next.run(ctx).await?;
            resp.set_header("x-ratelimit-limit", &result.limit.to_string());
            resp.set_header("x-ratelimit-remaining", &result.remaining.to_string());
            resp.set_header("x-ratelimit-reset", &reset_secs.to_string());
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reaper_evicts_expired_entries() {
        tokio::time::pause();

        let store = MemoryStore::with_reap_interval(std::time::Duration::from_secs(1));

        // Create an entry with a 2-second window
        let result = store
            .check_and_increment("test-key", 10, std::time::Duration::from_secs(2))
            .await;
        assert!(result.allowed);

        // Verify entry exists
        assert!(store.entries.contains_key("test-key"));

        // Advance past the window expiry + reap interval
        tokio::time::advance(std::time::Duration::from_secs(3)).await;

        // Yield to let the reaper task run
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        // Entry should have been reaped
        assert!(
            !store.entries.contains_key("test-key"),
            "Expired entry should be evicted by reaper"
        );
    }

    #[tokio::test]
    async fn reaper_stops_when_store_is_dropped() {
        tokio::time::pause();

        let store = MemoryStore::with_reap_interval(std::time::Duration::from_secs(1));
        let weak_entries = Arc::downgrade(&store.entries);

        // Store is alive — weak reference should upgrade
        assert!(weak_entries.upgrade().is_some());

        // Drop the store
        drop(store);

        // Advance time past the reap interval so the reaper task runs
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        // After the reaper runs, it should see the weak ref fail and exit
        // The entries Arc should now be fully dropped
        assert!(
            weak_entries.upgrade().is_none(),
            "Reaper should stop and release the Arc when store is dropped"
        );
    }

    #[tokio::test]
    async fn rate_limiting_works_with_arc_store() {
        let store = MemoryStore::new();
        let window = std::time::Duration::from_secs(60);

        // First request — allowed
        let r1 = store.check_and_increment("key", 2, window).await;
        assert!(r1.allowed);
        assert_eq!(r1.remaining, 1);

        // Second request — allowed
        let r2 = store.check_and_increment("key", 2, window).await;
        assert!(r2.allowed);
        assert_eq!(r2.remaining, 0);

        // Third request — rejected
        let r3 = store.check_and_increment("key", 2, window).await;
        assert!(!r3.allowed);
        assert_eq!(r3.remaining, 0);
    }

    #[tokio::test]
    async fn memory_store_allows_under_limit() {
        let store = MemoryStore::new();
        let result = store
            .check_and_increment("key1", 5, Duration::from_secs(60))
            .await;
        assert!(result.allowed);
        assert_eq!(result.limit, 5);
        assert_eq!(result.remaining, 4);
    }

    #[tokio::test]
    async fn memory_store_counts_up_to_limit() {
        let store = MemoryStore::new();
        for _ in 0..5 {
            store
                .check_and_increment("key1", 5, Duration::from_secs(60))
                .await;
        }
        let result = store
            .check_and_increment("key1", 5, Duration::from_secs(60))
            .await;
        assert!(!result.allowed);
        assert_eq!(result.remaining, 0);
    }

    #[tokio::test]
    async fn memory_store_separate_keys() {
        let store = MemoryStore::new();
        for _ in 0..5 {
            store
                .check_and_increment("key1", 5, Duration::from_secs(60))
                .await;
        }
        let result = store
            .check_and_increment("key2", 5, Duration::from_secs(60))
            .await;
        assert!(result.allowed);
        assert_eq!(result.remaining, 4);
    }

    #[tokio::test]
    async fn memory_store_resets_after_window() {
        let store = MemoryStore::new();
        // Fill up the limit
        for _ in 0..5 {
            store
                .check_and_increment("key1", 5, Duration::from_secs(0))
                .await;
        }
        // Window of 0 seconds means it should expire immediately on next check
        // We need a tiny sleep to let the Instant advance
        tokio::time::sleep(Duration::from_millis(5)).await;
        let result = store
            .check_and_increment("key1", 5, Duration::from_secs(0))
            .await;
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn memory_store_reset_clears_key() {
        let store = MemoryStore::new();
        for _ in 0..5 {
            store
                .check_and_increment("key1", 5, Duration::from_secs(60))
                .await;
        }
        store.reset("key1").await;
        let result = store
            .check_and_increment("key1", 5, Duration::from_secs(60))
            .await;
        assert!(result.allowed);
        assert_eq!(result.remaining, 4);
    }

    // `RateLimit::new()` constructs a `MemoryStore`, which now spawns a
    // background reaper via `tokio::spawn` and reads the clock via
    // `tokio::time::Instant::now()`. Both require an active Tokio runtime,
    // so these builder-state tests need `#[tokio::test]` rather than plain
    // `#[test]`.
    #[tokio::test]
    async fn rate_limit_builder_sets_limit_and_window() {
        let rl = RateLimit::new()
            .limit(100)
            .window(Duration::from_secs(900));
        // Verify through internal state — limit/window are stored correctly
        assert!(matches!(
            &rl.config,
            RateLimitConfig::Simple { limit: 100, .. }
        ));
    }

    #[tokio::test]
    async fn rate_limit_default_key_is_ip() {
        let rl = RateLimit::new();
        assert!(matches!(
            &rl.config,
            RateLimitConfig::Simple {
                key: RateKey::Ip,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rate_limit_tier_switches_to_tiered_mode() {
        let rl = RateLimit::new()
            .tier("free", 100, Duration::from_secs(3600))
            .tier("pro", 10_000, Duration::from_secs(3600));
        assert!(matches!(&rl.config, RateLimitConfig::Tiered { .. }));
    }

    use crate::app::App;
    use crate::request::Request;
    use http::StatusCode;

    #[tokio::test]
    async fn allows_requests_under_limit() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(3)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Ip),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        let resp = client.get("/").peer_ip("1.2.3.4").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.header("x-ratelimit-limit"), Some("3"));
        assert_eq!(resp.header("x-ratelimit-remaining"), Some("2"));
        assert!(resp.header("x-ratelimit-reset").is_some());
    }

    #[tokio::test]
    async fn rejects_at_limit_with_429() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(2)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Ip),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        client.get("/").peer_ip("1.2.3.4").send().await;
        client.get("/").peer_ip("1.2.3.4").send().await;
        let resp = client.get("/").peer_ip("1.2.3.4").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.header("retry-after").is_some());
    }

    #[tokio::test]
    async fn different_ips_get_separate_counters() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(1)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Ip),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        let resp1 = client.get("/").peer_ip("1.1.1.1").send().await;
        assert_eq!(resp1.status(), StatusCode::OK);

        let resp2 = client.get("/").peer_ip("2.2.2.2").send().await;
        assert_eq!(resp2.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn header_key_uses_header_value() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(1)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Header("x-api-key".into())),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        let resp = client
            .get("/")
            .header("x-api-key", "key-a")
            .send()
            .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = client
            .get("/")
            .header("x-api-key", "key-a")
            .send()
            .await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Different key → separate counter
        let resp = client
            .get("/")
            .header("x-api-key", "key-b")
            .send()
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn custom_key_function() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(1)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Custom(Arc::new(|_ctx| {
                        "everyone".to_string()
                    }))),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        let resp = client.get("/").peer_ip("1.1.1.1").send().await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Different IP but same custom key → shared counter
        let resp = client.get("/").peer_ip("2.2.2.2").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn tiered_limits_per_plan() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .tier("free", 1, Duration::from_secs(60))
                    .tier("pro", 100, Duration::from_secs(60))
                    .resolve_tier(|ctx: &Context| {
                        ctx.headers()
                            .get("x-plan")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("free")
                            .to_string()
                    }),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        // Free user hits limit after 1 request
        let resp = client.get("/").header("x-plan", "free").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = client.get("/").header("x-plan", "free").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Pro user still has headroom
        let resp = client.get("/").header("x-plan", "pro").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_tier_falls_back_to_first() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .tier("free", 1, Duration::from_secs(60))
                    .tier("pro", 100, Duration::from_secs(60))
                    .resolve_tier(|_ctx: &Context| "unknown_plan".to_string()),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        // Falls back to first tier (free, limit=1)
        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn rate_limit_429_has_json_body() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(1)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Ip),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        client.get("/").peer_ip("1.1.1.1").send().await;
        let resp = client.get("/").peer_ip("1.1.1.1").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = resp.text();
        assert!(body.contains("rate limit exceeded"));
        assert!(body.contains("retry_after"));
    }

    #[tokio::test]
    async fn scoped_rate_limit_on_group() {
        let client = App::test()
            .group("/api", |g| {
                g.use_mw(
                    RateLimit::new()
                        .limit(1)
                        .window(Duration::from_secs(60))
                        .key(RateKey::Ip),
                )
                .get("/data", |_: Request| "api")
            })
            .get("/page", |_: Request| "page")
            .into_client();

        // API route is rate limited
        client.get("/api/data").peer_ip("1.1.1.1").send().await;
        let resp = client.get("/api/data").peer_ip("1.1.1.1").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Page route is not rate limited
        let resp = client.get("/page").peer_ip("1.1.1.1").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_peer_ip_falls_back_to_unknown() {
        let client = App::test()
            .use_mw(
                RateLimit::new()
                    .limit(1)
                    .window(Duration::from_secs(60))
                    .key(RateKey::Ip),
            )
            .get("/", |_: Request| "ok")
            .into_client();

        // No peer_ip set — uses fallback key
        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
