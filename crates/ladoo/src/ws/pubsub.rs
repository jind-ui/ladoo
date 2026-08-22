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

/// Redis-backed pub/sub for multi-server deployments.
///
/// Uses a single Redis Pub/Sub channel (default: `ladoo:ws:broadcast`)
/// to distribute [`BroadcastEvent`]s across server instances. Each
/// server subscribes to this channel on startup and publishes to it
/// when a broadcast occurs.
///
/// Requires the `ws-redis` feature.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::ws::{ChannelRouter, RedisPubSub};
///
/// let pubsub = RedisPubSub::new("redis://127.0.0.1:6379")
///     .expect("Redis connection failed");
///
/// App::new()
///     .channel_with_pubsub(
///         ChannelRouter::new().route("chat:*", ChatChannel),
///         pubsub,
///     )
///     .run("0.0.0.0:3000");
/// ```
#[cfg(feature = "ws-redis")]
pub struct RedisPubSub {
    client: redis::Client,
    pub(crate) channel_name: String,
    instance_id: String,
}

/// Wire envelope wrapping a [`BroadcastEvent`] with the publishing
/// [`RedisPubSub`] instance's id.
///
/// A [`Broadcaster`](crate::ws::Broadcaster) delivers every broadcast to
/// its own local subscribers directly *and* publishes it to Redis so
/// other server instances can do the same. Because a server is typically
/// also subscribed to the same Redis channel it publishes on (so it can
/// receive broadcasts from *other* servers), it would otherwise receive
/// its own message back and deliver it to local clients twice. Tagging
/// each message with the sending instance's id lets [`RedisPubSub::subscribe`]
/// filter out that self-echo while still delivering messages from every
/// other instance.
#[cfg(feature = "ws-redis")]
#[derive(Serialize, Deserialize)]
struct RedisEnvelope {
    origin: String,
    event: BroadcastEvent,
}

#[cfg(feature = "ws-redis")]
impl RedisPubSub {
    /// Create a new RedisPubSub backend.
    ///
    /// # Errors
    ///
    /// Returns [`WsError::PubSub`] if `redis_url` cannot be parsed into a
    /// valid Redis connection URL. This does not attempt a connection —
    /// connections are established lazily on `publish`/`subscribe`.
    pub fn new(redis_url: &str) -> Result<Self, WsError> {
        let client = redis::Client::open(redis_url).map_err(|e| WsError::PubSub(e.to_string()))?;
        Ok(Self {
            client,
            channel_name: "ladoo:ws:broadcast".into(),
            instance_id: uuid::Uuid::new_v4().to_string(),
        })
    }

    /// Set the Redis channel name (default: `"ladoo:ws:broadcast"`).
    pub fn channel(mut self, name: &str) -> Self {
        self.channel_name = name.to_string();
        self
    }
}

#[cfg(feature = "ws-redis")]
#[async_trait]
impl PubSub for RedisPubSub {
    async fn publish(&self, event: BroadcastEvent) -> Result<(), WsError> {
        let envelope = RedisEnvelope {
            origin: self.instance_id.clone(),
            event,
        };
        let json =
            serde_json::to_string(&envelope).map_err(|e| WsError::Serialization(e.to_string()))?;

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| WsError::PubSub(e.to_string()))?;

        redis::cmd("PUBLISH")
            .arg(&self.channel_name)
            .arg(&json)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| WsError::PubSub(e.to_string()))?;

        Ok(())
    }

    async fn subscribe(&self) -> Result<PubSubReceiver, WsError> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|e| WsError::PubSub(e.to_string()))?;

        pubsub
            .subscribe(&self.channel_name)
            .await
            .map_err(|e| WsError::PubSub(e.to_string()))?;

        let instance_id = self.instance_id.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let envelope: RedisEnvelope = match serde_json::from_str(&payload) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                // Skip messages this same instance published — see
                // `RedisEnvelope`'s doc comment.
                if envelope.origin == instance_id {
                    continue;
                }
                if tx.send(envelope.event).is_err() {
                    break;
                }
            }
        });

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

    #[cfg(feature = "ws-redis")]
    mod redis_tests {
        use super::super::*;

        #[test]
        fn redis_pubsub_default_channel() {
            // Skip if no Redis URL
            let url = match std::env::var("REDIS_URL") {
                Ok(u) => u,
                Err(_) => return,
            };
            let ps = RedisPubSub::new(&url).unwrap();
            assert_eq!(ps.channel_name, "ladoo:ws:broadcast");
        }

        #[test]
        fn redis_pubsub_custom_channel() {
            let url = match std::env::var("REDIS_URL") {
                Ok(u) => u,
                Err(_) => return,
            };
            let ps = RedisPubSub::new(&url).unwrap().channel("custom:channel");
            assert_eq!(ps.channel_name, "custom:channel");
        }

        #[tokio::test]
        async fn redis_pubsub_publish_and_subscribe() {
            let url = match std::env::var("REDIS_URL") {
                Ok(u) => u,
                Err(_) => return,
            };

            let ps1 = RedisPubSub::new(&url).unwrap().channel("ladoo:test:pubsub");
            let ps2 = RedisPubSub::new(&url).unwrap().channel("ladoo:test:pubsub");

            let mut rx = ps2.subscribe().await.unwrap();

            // Small delay to let subscription establish
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            let event = BroadcastEvent {
                topic: "chat:lobby".into(),
                event: "test_msg".into(),
                payload: serde_json::json!({"n": 42}),
                exclude_socket: None,
            };
            ps1.publish(event).await.unwrap();

            let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("should receive within timeout")
                .expect("channel should not close");

            assert_eq!(received.topic, "chat:lobby");
            assert_eq!(received.event, "test_msg");
            assert_eq!(received.payload["n"], 42);
        }

        #[test]
        fn redis_pubsub_invalid_url() {
            let result = RedisPubSub::new("not-a-valid-url");
            assert!(result.is_err());
        }

        // A single `RedisPubSub` instance is both the publisher and one of
        // the subscribers whenever a Broadcaster forwards cross-server
        // messages back into its own local delivery channel (see
        // `Broadcaster::ensure_pubsub_forwarder`). Redis PUBLISH fans out
        // to every subscriber of a channel, including the publisher's own
        // subscription, so without de-duplication a server would
        // re-deliver its own broadcasts to its own local clients a second
        // time. `RedisPubSub` must filter out messages that originated
        // from itself.
        #[tokio::test]
        async fn redis_pubsub_does_not_echo_own_publish() {
            let url = match std::env::var("REDIS_URL") {
                Ok(u) => u,
                Err(_) => return,
            };

            let ps = RedisPubSub::new(&url)
                .unwrap()
                .channel("ladoo:test:no-echo");

            let mut rx = ps.subscribe().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            let event = BroadcastEvent {
                topic: "chat:lobby".into(),
                event: "self_echo".into(),
                payload: serde_json::json!({"n": 1}),
                exclude_socket: None,
            };
            ps.publish(event).await.unwrap();

            let result =
                tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
            assert!(
                result.is_err(),
                "should not receive its own published event back"
            );
        }
    }
}
