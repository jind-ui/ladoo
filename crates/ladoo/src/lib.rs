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
//! ## Configuration
//!
//! Define typed config structs with `#[derive(Config)]`. Values are
//! resolved from environment variables → TOML files → defaults:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! #[derive(Config)]
//! struct AppConfig {
//!     #[config(default = 3000)]
//!     port: u16,
//!     #[config(env = "DATABASE_URL")]
//!     database_url: String,
//!     pool_size: Option<u32>,
//! }
//!
//! App::new()
//!     .config::<AppConfig>()
//!     .get("/", |cfg: State<AppConfig>| {
//!         format!("Running on port {}", cfg.port)
//!     })
//!     .run("0.0.0.0:3000");
//! ```
//!
//! TOML files are read from `config/default.toml` and
//! `config/{environment}.toml` in the working directory. The
//! environment is detected from `LADOO_ENV` or `APP_ENV`.
//!
//! ## Logging & Request Tracing
//!
//! Structured logging is automatic. Dev mode gets pretty output,
//! prod mode gets JSON — detected from `LADOO_ENV`:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! App::new()
//!     .get("/", |_: Request| "hello")
//!     .run("0.0.0.0:3000");
//! // Dev:  INFO request{method=GET path=/ request_id=abc-123} ← 200 OK (1ms)
//! // Prod: {"timestamp":"...","level":"info","method":"GET","path":"/","status":200}
//! ```
//!
//! Every request gets a UUID request ID, propagated via the
//! `X-Request-Id` header and available as `State<RequestId>`:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! async fn handler(id: State<RequestId>) -> String {
//!     format!("request: {}", id.0)
//! }
//! ```
//!
//! Customize logging:
//!
//! ```rust,ignore
//! App::new()
//!     .log_level("debug")
//!     .request_id_header("x-trace-id")
//!     .get("/", handler)
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
//!
//! ## Testing
//!
//! Test your app in-memory without starting a server:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! #[tokio::test]
//! async fn returns_hello() {
//!     let client = App::test()
//!         .get("/", |_: Request| "hello")
//!         .into_client();
//!
//!     let resp = client.get("/").send().await;
//!     assert_eq!(resp.status(), 200);
//!     assert_eq!(resp.text(), "hello");
//! }
//! ```
//!
//! For integration tests over real TCP (requires `features = ["test-server"]`):
//!
//! ```rust,ignore
//! #[tokio::test]
//! async fn integration() {
//!     let server = App::test()
//!         .get("/", |_: Request| "hello")
//!         .spawn()
//!         .await;
//!
//!     let resp = server.get("/").send().await;
//!     assert_eq!(resp.text(), "hello");
//! }
//! ```
//!
//! ## Security Headers
//!
//! Opt-in secure response headers with sensible defaults:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! App::new()
//!     .use_mw(SecureHeaders::default())
//!     .get("/", |_: Request| "Hello")
//!     .run("0.0.0.0:3000");
//! ```
//!
//! Customize individual headers or disable them:
//!
//! ```rust,ignore
//! App::new().use_mw(
//!     SecureHeaders::new()
//!         .hsts("max-age=31536000")
//!         .x_frame_options(None)
//! )
//! ```
//!
//! ## Authentication
//!
//! Type-safe auth with compile-time enforcement. If a handler takes
//! `Auth<User>`, authentication is guaranteed:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! let api_key = ApiKeyAuth::new()
//!     .key("secret", User { name: "Alice".into() });
//!
//! App::new()
//!     .group("/api", |g| g
//!         .guard(api_key)
//!         .get("/me", |user: Auth<User>| user.name.clone())
//!     )
//! ```
//!
//! Optional auth with `Option<Auth<T>>`:
//!
//! ```rust,ignore
//! async fn home(user: Option<Auth<User>>) -> String {
//!     match user {
//!         Some(u) => format!("Welcome back, {}", u.name),
//!         None => "Welcome, guest".into(),
//!     }
//! }
//! ```
//!
//! ## Authorization (RBAC)
//!
//! Three levels: role check, permission check, resource policy:
//!
//! ```rust,ignore
//! let rbac = Rbac::new()
//!     .role("editor", &["posts:read", "posts:write"])
//!     .role("admin", &["*"]);
//!
//! App::new()
//!     .provide(rbac)
//!     .group("/admin", |g| g
//!         .guard(jwt)
//!         .use_mw(RequireRole::<Claims>::new("admin"))
//!         .get("/stats", admin_stats)
//!     )
//! ```
//!
//! ## Graceful Shutdown
//!
//! The server listens for SIGTERM and SIGINT and drains in-flight
//! requests before exiting (default 30-second timeout):
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//! use std::time::Duration;
//!
//! App::new()
//!     .shutdown_timeout(Duration::from_secs(60))
//!     .get("/", |_: Request| "Hello")
//!     .run("0.0.0.0:3000");
//! ```
//!
//! ## Plugins
//!
//! Package routes, state, middleware, and shutdown cleanup into
//! reusable components:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! struct HealthPlugin;
//!
//! impl Plugin for HealthPlugin {
//!     fn name(&self) -> &str { "health" }
//!
//!     fn register(self, app: App) -> App {
//!         app.get("/health", |_: Request| "ok")
//!     }
//! }
//!
//! App::new()
//!     .plugin(HealthPlugin)
//!     .run("0.0.0.0:3000");
//! ```
//!
//! Plugins can register middleware, provide state, add routes, and
//! schedule shutdown hooks. They run once at startup and have zero
//! per-request cost.
//!
//! ## CORS
//!
//! Allow cross-origin requests with a one-liner or custom policy:
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! // Permissive (development)
//! App::new()
//!     .use_mw(Cors::permissive())
//!     .get("/api/data", handler);
//!
//! // Custom (production)
//! App::new()
//!     .use_mw(
//!         Cors::new()
//!             .allow_origin("https://myapp.com")
//!             .allow_methods([Method::GET, Method::POST])
//!             .allow_headers(["Content-Type", "Authorization"])
//!     )
//!     .get("/api/data", handler);
//! ```
//!
//! ## Rate Limiting
//!
//! Protect endpoints from abuse with pluggable storage:
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
//!
//! Tiered limits by user plan:
//!
//! ```rust,ignore
//! App::new()
//!     .use_mw(
//!         RateLimit::new()
//!             .tier("free", 100, Duration::from_secs(3600))
//!             .tier("pro", 10_000, Duration::from_secs(3600))
//!             .resolve_tier(|ctx: &Context| {
//!                 ctx.headers()
//!                     .get("x-plan")
//!                     .and_then(|v| v.to_str().ok())
//!                     .unwrap_or("free")
//!                     .to_string()
//!             })
//!     )
//!     .get("/api/data", handler);
//! ```

pub mod app;
pub mod auth;
pub mod config;
pub mod context;
pub mod cors;
pub mod error;
pub mod extract;
pub mod handler;
#[cfg(feature = "logging")]
pub mod logging;
pub mod middleware;
#[cfg(feature = "json")]
pub mod pagination;
pub mod plugin;
pub mod prelude;
pub mod rate_limit;
pub mod request;
pub mod response;
pub mod router;
pub mod secure_headers;
pub(crate) mod shutdown;
pub mod state;
pub mod testing;
pub(crate) mod server;

/// Derive `Display` and `IntoResponse` for custom error enums.
///
/// See [`ladoo_macros::AppError`] for the attribute syntax.
#[cfg(feature = "macros")]
pub use ladoo_macros::AppError;

/// Derive `Config` for typed configuration loading.
///
/// See [`ladoo_macros::Config`] for the attribute syntax.
#[cfg(all(feature = "macros", feature = "config"))]
pub use ladoo_macros::Config;
