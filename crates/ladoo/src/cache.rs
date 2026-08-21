//! Pluggable caching with typed access.
//!
//! The [`CacheStore`] trait defines the backend interface (raw bytes).
//! [`MemoryStore`] is a DashMap-backed in-memory implementation with
//! lazy expiration. [`Cache`] wraps any store with typed
//! serialization and a [`remember()`](Cache::remember) convenience.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//! use ladoo::cache::MemoryStore;
//! use std::time::Duration;
//!
//! App::new()
//!     .provide(Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(300)))
//!     .get("/user", |cache: State<Cache>| async move {
//!         let user: Option<String> = cache.get("user:1").await?;
//!         Ok(user.unwrap_or_else(|| "not cached".into()))
//!     });
//! ```

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{Error, Result};

/// A pluggable cache backend operating on raw bytes.
///
/// Implement this trait to connect Redis, Memcached, or any other
/// cache backend. The framework ships [`MemoryStore`] for development
/// and small deployments.
///
/// All methods are async to support network-backed stores. In-memory
/// stores like [`MemoryStore`] return immediately.
#[async_trait]
pub trait CacheStore: Send + Sync + 'static {
    /// Retrieve a value by key.
    ///
    /// Returns `Ok(None)` if the key is missing or expired.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Store a value with an optional TTL.
    ///
    /// `None` TTL means the entry does not expire (lives until
    /// explicitly deleted or evicted by the backend).
    async fn set(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<()>;

    /// Delete a key. Returns `true` if the key existed.
    async fn delete(&self, key: &str) -> Result<bool>;

    /// Check if a key exists and is not expired.
    async fn has(&self, key: &str) -> Result<bool>;
}

struct CacheEntry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

/// In-memory cache store backed by [`DashMap`] with lazy expiration.
///
/// Expired entries are removed on the next read — no background
/// cleanup thread. There is no max-size cap; the store grows as
/// entries are added. For bounded caches with eviction policies,
/// use an external store like `moka` or Redis.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::cache::{CacheStore, MemoryStore};
///
/// let store = MemoryStore::new();
/// store.set("key", b"value".to_vec(), None).await?;
/// ```
pub struct MemoryStore {
    data: DashMap<String, CacheEntry>,
}

impl MemoryStore {
    /// Create a new empty in-memory cache store.
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CacheStore for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self.data.get(key) {
            Some(entry) => {
                if let Some(expires_at) = entry.expires_at {
                    if Instant::now() >= expires_at {
                        drop(entry);
                        self.data.remove(key);
                        return Ok(None);
                    }
                }
                Ok(Some(entry.value.clone()))
            }
            None => Ok(None),
        }
    }

    async fn set(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<()> {
        let expires_at = ttl.map(|d| Instant::now() + d);
        self.data
            .insert(key.to_string(), CacheEntry { value, expires_at });
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<bool> {
        Ok(self.data.remove(key).is_some())
    }

    async fn has(&self, key: &str) -> Result<bool> {
        match self.data.get(key) {
            Some(entry) => {
                if let Some(expires_at) = entry.expires_at {
                    if Instant::now() >= expires_at {
                        drop(entry);
                        self.data.remove(key);
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// Typed cache wrapper with serialization and `remember()`.
///
/// Wraps any [`CacheStore`] with `serde_json` serialization for
/// typed access. Configure a default TTL at construction; individual
/// operations can override it.
///
/// `Cache` is `Clone` (backed by `Arc<dyn CacheStore>`) and works
/// with Ladoo's `State<T>` pattern:
///
/// ```rust,ignore
/// use ladoo::prelude::*;
/// use ladoo::cache::MemoryStore;
/// use std::time::Duration;
///
/// App::new()
///     .provide(Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(300)))
///     .get("/data", |cache: State<Cache>| async move {
///         let value: Option<String> = cache.get("key").await?;
///         Ok(value.unwrap_or_default())
///     });
/// ```
#[derive(Clone)]
pub struct Cache {
    store: Arc<dyn CacheStore>,
    default_ttl: Option<Duration>,
}

impl Cache {
    /// Create a cache wrapping any [`CacheStore`] backend.
    pub fn new(store: impl CacheStore) -> Self {
        Self {
            store: Arc::new(store),
            default_ttl: None,
        }
    }

    /// Set the default TTL applied when `set` or `remember` receives `None`.
    ///
    /// Without a default, entries with `None` TTL live until deleted.
    pub fn default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(ttl);
        self
    }

    fn resolve_ttl(&self, ttl: Option<Duration>) -> Option<Duration> {
        ttl.or(self.default_ttl)
    }

    /// Get a typed value, deserializing from the store's raw bytes.
    ///
    /// Returns `Ok(None)` if the key is missing or expired.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.store.get(key).await? {
            Some(bytes) => {
                let value = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::internal("cache deserialization failed").with_source(e))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Store a typed value, serializing to bytes via `serde_json`.
    ///
    /// `ttl`: `None` uses the [`default_ttl`](Cache::default_ttl)
    /// (or no expiry if that is also `None`). `Some(duration)` overrides.
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| Error::internal("cache serialization failed").with_source(e))?;
        self.store.set(key, bytes, self.resolve_ttl(ttl)).await
    }

    /// Delete a key. Returns `true` if it existed.
    pub async fn delete(&self, key: &str) -> Result<bool> {
        self.store.delete(key).await
    }

    /// Check if a key exists and is not expired.
    pub async fn has(&self, key: &str) -> Result<bool> {
        self.store.has(key).await
    }

    /// Get-or-compute: returns the cached value on hit, or runs the
    /// closure, caches its result, and returns it on miss.
    ///
    /// If the closure returns `Err`, nothing is cached and the error
    /// propagates.
    ///
    /// `ttl`: `None` uses the [`default_ttl`](Cache::default_ttl).
    /// `Some(duration)` overrides.
    pub async fn remember<T, F, Fut>(&self, key: &str, ttl: Option<Duration>, f: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if let Some(cached) = self.get::<T>(key).await? {
            return Ok(cached);
        }
        let value = f().await?;
        self.set(key, &value, ttl).await?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_get_missing_key_returns_none() {
        let store = MemoryStore::new();
        let result = store.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn memory_store_set_and_get() {
        let store = MemoryStore::new();
        store.set("key", b"hello".to_vec(), None).await.unwrap();
        let result = store.get("key").await.unwrap();
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn memory_store_set_overwrites_existing() {
        let store = MemoryStore::new();
        store.set("key", b"first".to_vec(), None).await.unwrap();
        store.set("key", b"second".to_vec(), None).await.unwrap();
        let result = store.get("key").await.unwrap();
        assert_eq!(result, Some(b"second".to_vec()));
    }

    #[tokio::test]
    async fn memory_store_delete_existing_returns_true() {
        let store = MemoryStore::new();
        store.set("key", b"value".to_vec(), None).await.unwrap();
        assert!(store.delete("key").await.unwrap());
    }

    #[tokio::test]
    async fn memory_store_delete_missing_returns_false() {
        let store = MemoryStore::new();
        assert!(!store.delete("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn memory_store_delete_removes_entry() {
        let store = MemoryStore::new();
        store.set("key", b"value".to_vec(), None).await.unwrap();
        store.delete("key").await.unwrap();
        assert!(store.get("key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn memory_store_has_returns_true_for_existing() {
        let store = MemoryStore::new();
        store.set("key", b"value".to_vec(), None).await.unwrap();
        assert!(store.has("key").await.unwrap());
    }

    #[tokio::test]
    async fn memory_store_has_returns_false_for_missing() {
        let store = MemoryStore::new();
        assert!(!store.has("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn memory_store_expired_entry_returns_none() {
        let store = MemoryStore::new();
        store
            .set("key", b"value".to_vec(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(store.get("key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn memory_store_expired_entry_removed_on_get() {
        let store = MemoryStore::new();
        store
            .set("key", b"value".to_vec(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        store.get("key").await.unwrap();
        assert!(!store.data.contains_key("key"));
    }

    #[tokio::test]
    async fn memory_store_has_returns_false_for_expired() {
        let store = MemoryStore::new();
        store
            .set("key", b"value".to_vec(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!store.has("key").await.unwrap());
    }

    #[tokio::test]
    async fn memory_store_has_removes_expired_entry() {
        let store = MemoryStore::new();
        store
            .set("key", b"value".to_vec(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        store.has("key").await.unwrap();
        assert!(!store.data.contains_key("key"));
    }

    #[tokio::test]
    async fn memory_store_unexpired_entry_returned() {
        let store = MemoryStore::new();
        store
            .set("key", b"value".to_vec(), Some(Duration::from_secs(60)))
            .await
            .unwrap();
        let result = store.get("key").await.unwrap();
        assert_eq!(result, Some(b"value".to_vec()));
    }

    #[tokio::test]
    async fn memory_store_no_ttl_never_expires() {
        let store = MemoryStore::new();
        store.set("key", b"value".to_vec(), None).await.unwrap();
        assert!(store.has("key").await.unwrap());
        assert_eq!(store.get("key").await.unwrap(), Some(b"value".to_vec()));
    }

    #[tokio::test]
    async fn memory_store_multiple_keys_independent() {
        let store = MemoryStore::new();
        store.set("a", b"1".to_vec(), None).await.unwrap();
        store.set("b", b"2".to_vec(), None).await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), Some(b"1".to_vec()));
        assert_eq!(store.get("b").await.unwrap(), Some(b"2".to_vec()));
        store.delete("a").await.unwrap();
        assert!(store.get("a").await.unwrap().is_none());
        assert_eq!(store.get("b").await.unwrap(), Some(b"2".to_vec()));
    }

    #[tokio::test]
    async fn memory_store_default_trait() {
        let store = MemoryStore::default();
        assert!(store.get("key").await.unwrap().is_none());
    }

    #[test]
    fn memory_store_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MemoryStore>();
    }

    // --- Cache wrapper tests ---

    use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

    #[derive(Debug, Clone, PartialEq, SerdeSerialize, SerdeDeserialize)]
    struct TestUser {
        id: u64,
        name: String,
    }

    fn test_user() -> TestUser {
        TestUser {
            id: 42,
            name: "Alice".into(),
        }
    }

    #[tokio::test]
    async fn cache_get_missing_returns_none() {
        let cache = Cache::new(MemoryStore::new());
        let result: Option<String> = cache.get("missing").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cache_set_and_get_string() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("key", &"hello".to_string(), None).await.unwrap();
        let result: Option<String> = cache.get("key").await.unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn cache_set_and_get_struct() {
        let cache = Cache::new(MemoryStore::new());
        let user = test_user();
        cache.set("user:42", &user, None).await.unwrap();
        let result: Option<TestUser> = cache.get("user:42").await.unwrap();
        assert_eq!(result, Some(user));
    }

    #[tokio::test]
    async fn cache_set_and_get_integer() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("count", &100_u64, None).await.unwrap();
        let result: Option<u64> = cache.get("count").await.unwrap();
        assert_eq!(result, Some(100));
    }

    #[tokio::test]
    async fn cache_set_and_get_vec() {
        let cache = Cache::new(MemoryStore::new());
        let items = vec![1_u32, 2, 3];
        cache.set("items", &items, None).await.unwrap();
        let result: Option<Vec<u32>> = cache.get("items").await.unwrap();
        assert_eq!(result, Some(vec![1, 2, 3]));
    }

    #[tokio::test]
    async fn cache_delete_existing_returns_true() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("key", &"value", None).await.unwrap();
        assert!(cache.delete("key").await.unwrap());
    }

    #[tokio::test]
    async fn cache_delete_missing_returns_false() {
        let cache = Cache::new(MemoryStore::new());
        assert!(!cache.delete("missing").await.unwrap());
    }

    #[tokio::test]
    async fn cache_has_existing_returns_true() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("key", &"value", None).await.unwrap();
        assert!(cache.has("key").await.unwrap());
    }

    #[tokio::test]
    async fn cache_has_missing_returns_false() {
        let cache = Cache::new(MemoryStore::new());
        assert!(!cache.has("missing").await.unwrap());
    }

    #[tokio::test]
    async fn cache_default_ttl_applies_when_set_passes_none() {
        let cache = Cache::new(MemoryStore::new()).default_ttl(Duration::from_millis(1));
        cache.set("key", &"value", None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result: Option<String> = cache.get("key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cache_explicit_ttl_overrides_default() {
        let cache = Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(60));
        cache
            .set("key", &"value", Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result: Option<String> = cache.get("key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cache_no_default_ttl_and_none_means_no_expiry() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("key", &"forever", None).await.unwrap();
        let result: Option<String> = cache.get("key").await.unwrap();
        assert_eq!(result, Some("forever".to_string()));
    }

    #[tokio::test]
    async fn cache_remember_miss_calls_closure() {
        let cache = Cache::new(MemoryStore::new());
        let value = cache
            .remember("user:1", None, || async { Ok(test_user()) })
            .await
            .unwrap();
        assert_eq!(value, test_user());
    }

    #[tokio::test]
    async fn cache_remember_hit_skips_closure() {
        let cache = Cache::new(MemoryStore::new());
        cache.set("user:1", &test_user(), None).await.unwrap();
        let mut called = false;
        let value: TestUser = cache
            .remember("user:1", None, || {
                called = true;
                async { Ok(test_user()) }
            })
            .await
            .unwrap();
        assert_eq!(value, test_user());
        assert!(!called);
    }

    #[tokio::test]
    async fn cache_remember_stores_result() {
        let cache = Cache::new(MemoryStore::new());
        cache
            .remember::<TestUser, _, _>("user:1", None, || async { Ok(test_user()) })
            .await
            .unwrap();
        let cached: Option<TestUser> = cache.get("user:1").await.unwrap();
        assert_eq!(cached, Some(test_user()));
    }

    #[tokio::test]
    async fn cache_remember_error_does_not_cache() {
        let cache = Cache::new(MemoryStore::new());
        let result = cache
            .remember::<String, _, _>("key", None, || async {
                Err(Error::internal("computation failed"))
            })
            .await;
        assert!(result.is_err());
        assert!(!cache.has("key").await.unwrap());
    }

    #[tokio::test]
    async fn cache_remember_uses_default_ttl() {
        let cache = Cache::new(MemoryStore::new()).default_ttl(Duration::from_millis(1));
        cache
            .remember::<String, _, _>("key", None, || async { Ok("value".into()) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result: Option<String> = cache.get("key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cache_remember_explicit_ttl_overrides() {
        let cache = Cache::new(MemoryStore::new()).default_ttl(Duration::from_secs(60));
        cache
            .remember::<String, _, _>("key", Some(Duration::from_millis(1)), || async {
                Ok("value".into())
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result: Option<String> = cache.get("key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cache_is_clone() {
        let cache = Cache::new(MemoryStore::new());
        let cache2 = cache.clone();
        cache.set("key", &"value", None).await.unwrap();
        let result: Option<String> = cache2.get("key").await.unwrap();
        assert_eq!(result, Some("value".to_string()));
    }

    #[test]
    fn cache_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Cache>();
    }

    #[tokio::test]
    async fn cache_get_deserialization_error() {
        let store = MemoryStore::new();
        store
            .set("bad", b"not valid json".to_vec(), None)
            .await
            .unwrap();
        let cache = Cache::new(store);
        let result = cache.get::<TestUser>("bad").await;
        assert!(result.is_err());
    }
}
