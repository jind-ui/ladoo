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
pub use crate::extract::FromRequest;
#[cfg(feature = "json")]
pub use crate::extract::Json;
#[cfg(feature = "json")]
pub use crate::extract::Query;
pub use crate::handler::IntoHandler;
pub use crate::request::Request;
pub use crate::response::{Html, IntoResponse, Response};
pub use http::StatusCode;
