//! Application state and dependency injection.
//!
//! Register values at startup with `App::provide` and extract them in
//! handlers with [`State`]. Any type that is `Send + Sync + 'static`
//! can be provided — database pools, configuration, API clients, etc.
//!
//! # Examples
//!
//! ```
//! use ladoo::state::State;
//!
//! let state = State(42_u32);
//! assert_eq!(*state, 42);
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A type-keyed map for storing application state.
///
/// Values are keyed by their concrete type — each type can appear at most
/// once. Used internally by the framework; users interact through
/// `App::provide` and [`State`].
// Not yet wired into `App` (that lands when `.provide`/`.run` are added),
// so clippy sees no call sites outside tests.
#[allow(dead_code)]
pub(crate) struct TypeMap {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

#[allow(dead_code)]
impl TypeMap {
    /// Create an empty type map.
    pub(crate) fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Insert a value, replacing any previous value of the same type.
    pub(crate) fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Get a reference to a value by type.
    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
    }

    /// Check whether a value of the given type is stored.
    pub(crate) fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }
}

/// Extract a shared value from application state.
///
/// `State<T>` is a handler argument that retrieves a value previously
/// registered with `App::provide`. The value is shared (behind an `Arc`)
/// across all requests.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// struct Database { /* ... */ }
///
/// App::new()
///     .provide(Database::connect(url).await?)
///     .get("/users", |db: State<Database>| {
///         // db derefs to &Database
///         format!("connected: {}", db.is_connected())
///     })
///     .run("0.0.0.0:3000");
/// ```
pub struct State<T>(pub T);

impl<T> std::ops::Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> State<T> {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut map = TypeMap::new();
        map.insert(42_u32);
        assert_eq!(map.get::<u32>(), Some(&42));
    }

    #[test]
    fn get_returns_none_for_missing_type() {
        let map = TypeMap::new();
        assert_eq!(map.get::<u32>(), None);
    }

    #[test]
    fn insert_replaces_existing() {
        let mut map = TypeMap::new();
        map.insert(1_u32);
        map.insert(2_u32);
        assert_eq!(map.get::<u32>(), Some(&2));
    }

    #[test]
    fn different_types_dont_conflict() {
        let mut map = TypeMap::new();
        map.insert(42_u32);
        map.insert("hello");
        assert_eq!(map.get::<u32>(), Some(&42));
        assert_eq!(map.get::<&str>(), Some(&"hello"));
    }

    #[test]
    fn contains_returns_true_for_present_type() {
        let mut map = TypeMap::new();
        map.insert(42_u32);
        assert!(map.contains::<u32>());
    }

    #[test]
    fn contains_returns_false_for_missing_type() {
        let map = TypeMap::new();
        assert!(!map.contains::<u32>());
    }

    #[test]
    fn works_with_custom_struct() {
        #[derive(Debug, PartialEq)]
        struct MyConfig {
            port: u16,
        }
        let mut map = TypeMap::new();
        map.insert(MyConfig { port: 3000 });
        assert_eq!(map.get::<MyConfig>(), Some(&MyConfig { port: 3000 }));
    }

    #[test]
    fn works_with_string() {
        let mut map = TypeMap::new();
        map.insert(String::from("database_url"));
        assert_eq!(map.get::<String>(), Some(&String::from("database_url")));
    }

    #[test]
    fn state_deref_accesses_inner() {
        let state = State(42_u32);
        assert_eq!(*state, 42);
    }

    #[test]
    fn state_into_inner_returns_value() {
        let state = State(String::from("hello"));
        let inner = state.into_inner();
        assert_eq!(inner, "hello");
    }
}
