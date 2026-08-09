//! Handler trait and conversion utilities.
//!
//! Handlers are stored as `Box<dyn Handler>` for fast compilation.
//! Both sync and async closures are supported via the IntoHandler trait.
