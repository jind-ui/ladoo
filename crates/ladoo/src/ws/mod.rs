//! WebSocket and Channel support.
//!
//! Feature-gated behind `ws`. Provides raw WebSocket connections and
//! Phoenix-style Channels with topic routing and broadcasting.

pub mod broadcaster;
pub mod channel;
pub(crate) mod connection;
pub mod error;
pub mod pubsub;
pub mod router;
pub mod socket;
pub(crate) mod upgrade;

pub use broadcaster::Broadcaster;
pub use channel::{Channel, ChannelContext, Reply};
pub use error::WsError;
#[cfg(feature = "ws-redis")]
pub use pubsub::RedisPubSub;
pub use pubsub::{BroadcastEvent, MemoryPubSub, PubSub, PubSubReceiver};
pub use router::ChannelRouter;
pub use socket::{Message, WebSocket};
pub use upgrade::websocket;
