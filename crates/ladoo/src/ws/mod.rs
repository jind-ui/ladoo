//! WebSocket and Channel support.
//!
//! Feature-gated behind `ws`. Provides raw WebSocket connections and
//! Phoenix-style Channels with topic routing and broadcasting.

pub mod error;
pub mod socket;
pub(crate) mod upgrade;

pub use error::WsError;
pub use socket::{Message, WebSocket};
