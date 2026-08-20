//! Re-exports of the most commonly used types.
//!
//! Import everything with `use ladoo::prelude::*` to get started quickly.
//! This brings [`App`], [`Request`], [`Response`], extractors, and HTTP
//! types into scope.
//!
//! # Examples
//!
//! ```
//! use ladoo::prelude::*;
//!
//! let app = App::new().get("/", |_req: Request| "Hello");
//! ```

pub use crate::app::App;
#[cfg(feature = "macros")]
pub use crate::AppError;
#[cfg(feature = "config")]
pub use crate::config::{Config, ConfigError};
#[cfg(all(feature = "macros", feature = "config"))]
pub use crate::Config;
pub use crate::context::Context;
pub use crate::config::Environment;
#[cfg(feature = "cors")]
pub use crate::cors::Cors;
pub use crate::error::{Error, Result};
pub use crate::extract::FromRequest;
pub use crate::response::{Html, IntoResponse, Response};
pub use crate::handler::IntoHandler;
pub use crate::health::{HealthCheckable, HealthConfig};
#[cfg(feature = "cache")]
pub use crate::cache::{Cache, CacheStore};
#[cfg(feature = "jobs")]
pub use crate::job::{BackoffStrategy, Job, JobConfig, JobContext, JobError, JobHandle, JobRunner};
#[cfg(all(feature = "macros", feature = "jobs"))]
pub use crate::Job;
#[cfg(feature = "json")]
pub use crate::extract::Json;
#[cfg(feature = "json")]
pub use crate::pagination::{
    CursorMeta, CursorPage, CursorParams, Page, PageMeta, Paginate, PaginationConfig,
};
pub use crate::middleware::{Middleware, Next};
pub use crate::plugin::Plugin;
#[cfg(feature = "rate-limit")]
pub use crate::rate_limit::{MemoryStore, RateKey, RateLimit, RateStore};
#[cfg(feature = "json")]
pub use crate::extract::Query;
#[cfg(feature = "json")]
pub use crate::extract::Path;
pub use crate::extract::{Valid, Validate};
#[cfg(feature = "auth")]
pub use crate::auth::{Auth, AuthError, AuthProvider, HasRole};
#[cfg(feature = "auth")]
pub use crate::auth::providers::ApiKeyAuth;
#[cfg(feature = "auth")]
pub use crate::auth::rbac::{Rbac, RequireRole, RequirePermission, ResourcePolicy};
#[cfg(feature = "auth-jwt")]
pub use crate::auth::providers::JwtAuth;
pub use async_trait::async_trait;
pub use crate::request::Request;
#[cfg(feature = "logging")]
pub use crate::logging::RequestId;
pub use crate::router::Router;
#[cfg(feature = "secure-headers")]
pub use crate::secure_headers::SecureHeaders;
pub use crate::state::State;
pub use http::StatusCode;
pub use crate::testing::TestClient;
