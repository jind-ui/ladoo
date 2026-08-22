//! PubSub trait for distributing channel messages across servers.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::error::WsError;

/// A broadcast event targeting a specific topic.
///
/// This is the wire format used both for local (in-process) delivery via
/// [`crate::ws::Broadcaster`] and for cross-server delivery via a
/// [`PubSub`] backend (e.g. Redis). It is `Serialize`/`Deserialize` so a
/// `PubSub` implementation can encode it for transport (JSON over a Redis
/// channel, for example) and decode it again on the receiving server.
///
/// # Examples
///
/// ```
/// use ladoo::ws::BroadcastEvent;
///
/// let event = BroadcastEvent {
///     topic: "chat:lobby".into(),
///     event: "new_msg".into(),
///     payload: serde_json::json!({"text": "hello"}),
///     exclude_socket: None,
/// };
/// let json = serde_json::to_string(&event).unwrap();
/// let round_tripped: BroadcastEvent = serde_json::from_str(&json).unwrap();
/// assert_eq!(round_tripped.topic, "chat:lobby");
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BroadcastEvent {
    /// The topic this message targets (e.g., "chat:lobby").
    pub topic: String,
    /// The event name (e.g., "new_msg").
    pub event: String,
    /// The message payload.
    pub payload: serde_json::Value,
    /// Socket ID to exclude from delivery (prevents echo to sender).
    pub exclude_socket: Option<String>,
}

/// Receiver for cross-server broadcast events.
///
/// Yielded by [`PubSub::subscribe`]. Each item is a [`BroadcastEvent`]
/// published by another server instance (or, for [`MemoryPubSub`], never
/// yields anything at all).
pub type PubSubReceiver = tokio::sync::mpsc::UnboundedReceiver<BroadcastEvent>;

/// Backend for distributing channel messages across server instances.
///
/// A `PubSub` implementation is responsible only for *cross-server*
/// fan-out — delivery to WebSocket clients connected to the same
/// process is handled separately by [`crate::ws::Broadcaster`]'s
/// in-process broadcast channel. Implementations must be cheap to clone
/// behind an `Arc` and safe to call concurrently from many tasks.
#[async_trait]
pub trait PubSub: Send + Sync + 'static {
    /// Publish a message to all servers subscribed to this topic.
    async fn publish(&self, event: BroadcastEvent) -> Result<(), WsError>;

    /// Subscribe to messages from other servers.
    async fn subscribe(&self) -> Result<PubSubReceiver, WsError>;
}

/// In-process pub/sub — no cross-server distribution.
///
/// Default when no Redis URL is configured. The `publish` method is a
/// no-op because in-process delivery is handled by
/// `tokio::sync::broadcast` in the Broadcaster.
///
/// # Examples
///
/// ```
/// use ladoo::ws::{BroadcastEvent, MemoryPubSub, PubSub};
///
/// # #[tokio::main]
/// # async fn main() {
/// let ps = MemoryPubSub;
/// let event = BroadcastEvent {
///     topic: "chat:lobby".into(),
///     event: "msg".into(),
///     payload: serde_json::json!({}),
///     exclude_socket: None,
/// };
/// assert!(ps.publish(event).await.is_ok());
/// # }
/// ```
pub struct MemoryPubSub;

#[async_trait]
impl PubSub for MemoryPubSub {
    async fn publish(&self, _event: BroadcastEvent) -> Result<(), WsError> {
        Ok(())
    }

    async fn subscribe(&self) -> Result<PubSubReceiver, WsError> {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_pubsub_publish_is_noop() {
        let ps = MemoryPubSub;
        let event = BroadcastEvent {
            topic: "test:topic".into(),
            event: "msg".into(),
            payload: serde_json::json!({"x": 1}),
            exclude_socket: None,
        };
        assert!(ps.publish(event).await.is_ok());
    }

    #[tokio::test]
    async fn memory_pubsub_subscribe_returns_dead_receiver() {
        let ps = MemoryPubSub;
        let mut rx = ps.subscribe().await.unwrap();
        // Should return None immediately since sender is dropped
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn broadcast_event_serializes() {
        let event = BroadcastEvent {
            topic: "chat:lobby".into(),
            event: "new_msg".into(),
            payload: serde_json::json!({"text": "hi"}),
            exclude_socket: Some("abc123".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: BroadcastEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.topic, "chat:lobby");
        assert_eq!(deserialized.event, "new_msg");
        assert_eq!(deserialized.exclude_socket.as_deref(), Some("abc123"));
    }

    #[test]
    fn pubsub_trait_is_object_safe() {
        fn assert_object_safe(_: &dyn PubSub) {}
        assert_object_safe(&MemoryPubSub);
    }
}
