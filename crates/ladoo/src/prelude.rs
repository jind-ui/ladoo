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
pub use crate::error::{Error, Result};
pub use crate::extract::FromRequest;
pub use crate::response::{Html, IntoResponse, Response};
pub use crate::handler::IntoHandler;
#[cfg(feature = "json")]
pub use crate::extract::Json;
pub use crate::middleware::{Middleware, Next};
#[cfg(feature = "json")]
pub use crate::extract::Query;
#[cfg(feature = "logging")]
pub use crate::logging::RequestId;
pub use crate::request::Request;
pub use crate::router::Router;
pub use crate::state::State;
pub use http::StatusCode;
pub use crate::testing::TestClient;
