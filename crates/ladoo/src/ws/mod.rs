//! WebSocket and Channel support.
//!
//! Feature-gated behind `ws`. Provides raw WebSocket connections and
//! Phoenix-style Channels with topic routing and broadcasting.

pub mod broadcaster;
pub mod error;
pub mod pubsub;
pub mod socket;
pub(crate) mod upgrade;

pub use broadcaster::Broadcaster;
pub use error::WsError;
pub use pubsub::{BroadcastEvent, MemoryPubSub, PubSub, PubSubReceiver};
pub use socket::{Message, WebSocket};
pub use upgrade::websocket;
