//! Rate limiting middleware with pluggable storage.
//!
//! The `RateLimit` middleware (added in a later phase task) tracks request
//! counts per key (IP, header, custom function) and rejects excess requests
//! with `429 Too Many Requests`.
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

use std::time::{Duration, Instant};

use dashmap::DashMap;

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
    pub reset_at: Instant,
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

/// In-memory rate limit store backed by a concurrent hash map.
///
/// Suitable for single-process applications. Expired entries are
/// cleaned lazily on access — no background reaper thread.
///
/// For distributed deployments, implement [`RateStore`] for a shared
/// backend like Redis.
pub struct MemoryStore {
    entries: DashMap<String, (u64, Instant)>,
}

impl MemoryStore {
    /// Create a new empty memory store.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
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
        let now = Instant::now();

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
        let reset_at = *expires;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
