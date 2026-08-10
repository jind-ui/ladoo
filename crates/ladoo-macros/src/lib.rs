#![warn(missing_docs)]

//! Procedural macros for the Ladoo framework.
//!
//! This crate provides `#[derive(AppError)]`, `#[derive(Config)]`,
//! and other derive macros used by Ladoo.

mod app_error;
mod config;

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
