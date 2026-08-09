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
//! ```rust,no_run
//! use ladoo::prelude::*;
//!
//! fn main() {
//!     App::new()
//!         .get("/", |_: Request| "Hello World")
//!         .run("0.0.0.0:3000");
//! }
//! ```
//!
//! ## Path Parameters
//!
//! ```rust,no_run
//! use ladoo::prelude::*;
//!
//! fn main() {
//!     App::new()
//!         .get("/users/:id", |req: Request| {
//!             let id = req.param("id").unwrap_or("0");
//!             format!("User {id}")
//!         })
//!         .run("0.0.0.0:3000");
//! }
//! ```
//!
//! ## JSON API
//!
//! ```rust,no_run,ignore
//! use ladoo::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize)]
//! struct CreateUser {
//!     name: String,
//! }
//!
//! #[derive(Serialize)]
//! struct User {
//!     id: u64,
//!     name: String,
//! }
//!
//! fn main() {
//!     App::new()
//!         .post("/users", |body: Json<CreateUser>| {
//!             Json(User { id: 1, name: body.0.name.clone() })
//!         })
//!         .run("0.0.0.0:3000");
//! }
//! ```
//!
//! ## Query Parameters
//!
//! ```rust,no_run,ignore
//! use ladoo::prelude::*;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Search {
//!     q: String,
//!     page: Option<u32>,
//! }
//!
//! fn main() {
//!     App::new()
//!         .get("/search", |params: Query<Search>| {
//!             format!("Searching for: {}", params.q)
//!         })
//!         .run("0.0.0.0:3000");
//! }
//! ```

pub mod app;
pub mod error;
pub mod extract;
pub mod handler;
pub mod prelude;
pub mod request;
pub mod response;
pub mod router;
pub(crate) mod server;
