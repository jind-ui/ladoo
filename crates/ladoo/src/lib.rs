#![warn(missing_docs)]

//! # Ladoo
//!
//! A Rust backend framework prioritizing simplicity and developer experience.
//!
//! Ladoo is designed around progressive disclosure: start with 5 lines,
//! grow to modules when you need them, and get full control when you want it.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! fn main() {
//!     App::new()
//!         .get("/", |_| "Hello World")
//!         .run("0.0.0.0:3000");
//! }
//! ```

pub mod app;
pub mod handler;
pub mod prelude;
pub mod request;
pub mod response;
pub mod router;
pub mod server;
