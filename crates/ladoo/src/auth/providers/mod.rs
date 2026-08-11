//! Built-in authentication providers.
//!
//! [`ApiKeyAuth`] authenticates via a header/key lookup. `JwtAuth`
//! (behind the `auth-jwt` feature) decodes and validates JSON Web Tokens.

mod api_key;

pub use api_key::ApiKeyAuth;

#[cfg(feature = "auth-jwt")]
mod jwt;

#[cfg(feature = "auth-jwt")]
pub use jwt::JwtAuth;
