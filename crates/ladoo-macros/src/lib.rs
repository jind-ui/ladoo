#![warn(missing_docs)]

//! Procedural macros for the Ladoo framework.
//!
//! This crate provides `#[derive(AppError)]`, `#[derive(Config)]`,
//! and other derive macros used by Ladoo.

mod app_error;
mod config;
mod job;

/// Derive `Display` and `IntoResponse` for error enums.
///
/// Each variant can carry an `#[error(status = NNN, message = "...")]`
/// attribute. Both fields are optional: `status` defaults to `500` and
/// `message` defaults to the variant's name.
///
/// # Examples
///
/// ```ignore
/// use ladoo::prelude::*;
///
/// #[derive(Debug, AppError)]
/// enum UserError {
///     #[error(status = 404, message = "user not found")]
///     NotFound,
///     #[error(status = 409)]
///     Conflict(String),
/// }
/// ```
#[proc_macro_derive(AppError, attributes(error))]
pub fn derive_app_error(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    app_error::derive(input.into()).into()
}

/// Derive `Config` for configuration structs.
///
/// Generates a [`Config::load()`] implementation that reads from
/// environment variables, TOML files (`config/default.toml` and
/// `config/{env}.toml`), and field defaults.
///
/// Requires the `config` feature on the `ladoo` crate (enabled by default).
///
/// # Field Attributes
///
/// - `#[config(default = <literal>)]` — fallback value if not found elsewhere
/// - `#[config(env = "<VAR>")]` — environment variable to check first
/// - Both can be combined; env var wins over TOML, TOML wins over default
/// - Fields with neither attribute are required (must be in TOML or error)
/// - `Option<T>` fields return `None` if missing (no default needed)
///
/// # Examples
///
/// ```ignore
/// use ladoo::prelude::*;
///
/// #[derive(Config)]
/// struct AppConfig {
///     #[config(default = 3000)]
///     port: u16,
///     #[config(env = "DATABASE_URL")]
///     database_url: String,
///     pool_size: Option<u32>,
/// }
/// ```
#[proc_macro_derive(Config, attributes(config))]
pub fn derive_config(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    config::derive(input.into()).into()
}

/// Derive `Job` for background job structs.
///
/// Generates `name()` (from struct name, snake_cased) and `config()`
/// (from `#[job(...)]` attributes). The user writes `handle()` as an
/// inherent method; the macro delegates to it.
///
/// # Attributes
///
/// - `retries = N` — max retries (default: 0)
/// - `timeout = "duration"` — `"30s"`, `"5m"`, `"1h"` (default: `"30s"`)
/// - `backoff = "strategy"` — `"fixed"` or `"exponential"` (default: `"exponential"`)
///
/// # Examples
///
/// ```ignore
/// use ladoo::prelude::*;
///
/// #[derive(Job)]
/// #[job(retries = 3, timeout = "5m")]
/// struct SendEmail { user_id: i64 }
///
/// impl SendEmail {
///     async fn handle(&self, ctx: &JobContext) -> Result<(), JobError> {
///         Ok(())
///     }
/// }
/// ```
#[proc_macro_derive(Job, attributes(job))]
pub fn derive_job(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    job::derive(input.into()).into()
}
