//! Built-in authentication providers.
//!
//! `ApiKeyAuth` (this module) and `JwtAuth` (behind the `auth-jwt`
//! feature) are implemented in later tasks; this module currently
//! only declares their placeholder files.

mod api_key;

#[cfg(feature = "auth-jwt")]
mod jwt;

#[cfg(feature = "auth-jwt")]
pub use jwt::JwtAuth;
