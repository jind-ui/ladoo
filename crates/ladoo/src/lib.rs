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
//!
//! ## State & Dependency Injection
//!
//! Register any `Send + Sync + 'static` value at startup and extract
//! it in handlers with [`State<T>`](state::State):
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! App::new()
//!     .provide(Database::connect(url).await?)
//!     .provide(AppConfig::from_file("config.toml")?)
//!     .get("/users/:id", |db: State<Database>, req: Request| {
//!         let id = req.param("id").unwrap();
//!         db.find_user(id)
//!     })
//!     .run("0.0.0.0:3000");
//! ```
//!
//! ## Middleware
//!
//! Middleware functions wrap the request/response pipeline. Each
//! middleware receives an owned [`Context`](context::Context) and a
//! [`Next`](middleware::Next) continuation:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! async fn logger(ctx: Context, next: Next) -> Result<Response> {
//!     let method = ctx.method().to_string();
//!     let path = ctx.path().to_string();
//!     let start = std::time::Instant::now();
//!     let resp = next.run(ctx).await?;
//!     println!("{method} {path} → {} ({}ms)",
//!         resp.status(), start.elapsed().as_millis());
//!     Ok(resp)
//! }
//!
//! App::new()
//!     .use_mw(logger)
//!     .get("/", |_: Request| "Hello");
//! ```
//!
//! ## Route Groups & Mounting
//!
//! Group routes under a shared prefix with scoped middleware:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! App::new()
//!     .group("/admin", |r| {
//!         r.use_mw(auth_middleware)
//!          .get("/dashboard", dashboard)
//!          .get("/settings", settings)
//!     })
//!     .mount("/api", api_routes());
//! ```

pub mod app;
pub mod context;
pub mod error;
pub mod extract;
pub mod handler;
pub mod middleware;
pub mod prelude;
pub mod request;
pub mod response;
pub mod router;
pub mod state;
pub mod testing;
pub(crate) mod server;

/// Derive `Display` and `IntoResponse` for custom error enums.
///
/// See [`ladoo_macros::AppError`] for the attribute syntax.
#[cfg(feature = "macros")]
pub use ladoo_macros::AppError;
