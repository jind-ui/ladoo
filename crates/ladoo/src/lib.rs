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
//!
//! ## Error Handling
//!
//! Ladoo provides three levels of error control:
//!
//! ### Level 1: Auto-500
//!
//! Use `?` on any `std::error::Error` — it automatically becomes a 500.
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! fn get_user(req: Request, db: State<Database>) -> Result<Json<User>> {
//!     let user = db.find_user(req.param("id").unwrap())?; // auto-500 on DB error
//!     Ok(Json(user))
//! }
//! ```
//!
//! ### Level 2: Controlled Status Codes
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! fn get_user(req: Request, db: State<Database>) -> Result<Json<User>> {
//!     let id = req.param("id").unwrap();
//!     let user = db.find_user(id)
//!         .map_err(|_| Error::not_found("user not found"))?;
//!     Ok(Json(user))
//! }
//! ```
//!
//! ### Level 3: Domain Errors
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! #[derive(Debug, AppError)]
//! enum UserError {
//!     #[error(status = 404, message = "user not found")]
//!     NotFound,
//!     #[error(status = 409, message = "email already taken")]
//!     DuplicateEmail,
//! }
//!
//! fn create_user(body: Json<NewUser>) -> std::result::Result<Json<User>, UserError> {
//!     // Return UserError variants directly
//!     Err(UserError::DuplicateEmail)
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
pub mod state;
pub(crate) mod server;

/// Derive `Display` and `IntoResponse` for custom error enums.
///
/// See [`ladoo_macros::AppError`] for the attribute syntax.
#[cfg(feature = "macros")]
pub use ladoo_macros::AppError;
