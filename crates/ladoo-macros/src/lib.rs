#![warn(missing_docs)]

//! Procedural macros for the Ladoo framework.
//!
//! This crate provides `#[derive(AppError)]`, `#[derive(Config)]`,
//! and other derive macros used by Ladoo.

mod app_error;

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
