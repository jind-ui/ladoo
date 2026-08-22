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
#[cfg(feature = "auth")]
pub use crate::auth::providers::ApiKeyAuth;
#[cfg(feature = "auth-jwt")]
pub use crate::auth::providers::JwtAuth;
#[cfg(feature = "auth")]
pub use crate::auth::rbac::{Rbac, RequirePermission, RequireRole, ResourcePolicy};
#[cfg(feature = "auth")]
pub use crate::auth::{Auth, AuthError, AuthProvider, HasRole};
#[cfg(feature = "cache")]
pub use crate::cache::{Cache, CacheStore};
pub use crate::config::Environment;
#[cfg(feature = "config")]
pub use crate::config::{Config, ConfigError};
pub use crate::context::Context;
#[cfg(feature = "cors")]
pub use crate::cors::Cors;
pub use crate::error::{Error, Result};
pub use crate::extract::FromRequest;
#[cfg(feature = "json")]
pub use crate::extract::Json;
#[cfg(feature = "json")]
pub use crate::extract::Path;
#[cfg(feature = "json")]
pub use crate::extract::Query;
pub use crate::extract::{Valid, Validate};
pub use crate::handler::IntoHandler;
pub use crate::health::{HealthCheckable, HealthConfig};
#[cfg(feature = "jobs")]
pub use crate::job::{BackoffStrategy, Job, JobConfig, JobContext, JobError, JobHandle, JobRunner};
#[cfg(feature = "logging")]
pub use crate::logging::RequestId;
pub use crate::middleware::{Middleware, Next};
#[cfg(feature = "json")]
pub use crate::pagination::{
    CursorMeta, CursorPage, CursorParams, Page, PageMeta, Paginate, PaginationConfig,
};
pub use crate::plugin::Plugin;
#[cfg(feature = "rate-limit")]
pub use crate::rate_limit::{MemoryStore, RateKey, RateLimit, RateStore};
pub use crate::request::Request;
pub use crate::response::{Html, IntoResponse, Response};
pub use crate::router::Router;
#[cfg(feature = "secure-headers")]
pub use crate::secure_headers::SecureHeaders;
pub use crate::state::State;
pub use crate::testing::TestClient;
#[cfg(feature = "ws")]
pub use crate::ws::{
    websocket, Broadcaster, Channel, ChannelContext, ChannelRouter, Message, Reply, WebSocket,
    WsError,
};
#[cfg(feature = "macros")]
pub use crate::AppError;
#[cfg(all(feature = "macros", feature = "config"))]
pub use crate::Config;
#[cfg(all(feature = "macros", feature = "jobs"))]
pub use crate::Job;
pub use async_trait::async_trait;
pub use http::StatusCode;
