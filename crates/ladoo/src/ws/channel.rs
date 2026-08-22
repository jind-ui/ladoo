//! Channel trait and related types for Phoenix-style topic routing.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::broadcaster::Broadcaster;
use crate::state::TypeMap;

/// A WebSocket channel — handles messages for a topic pattern.
///
/// Implement this trait to create a channel handler. The channel
/// system manages subscriptions, message routing, and cleanup.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::ws::{Channel, ChannelContext, Reply};
///
/// struct ChatChannel;
///
/// #[async_trait::async_trait]
/// impl Channel for ChatChannel {
///     async fn join(
///         &self,
///         topic: &str,
///         _payload: serde_json::Value,
///         _ctx: &ChannelContext,
///     ) -> Result<serde_json::Value, String> {
///         Ok(serde_json::json!({"joined": topic}))
///     }
///
///     async fn handle(
///         &self,
///         event: &str,
///         payload: serde_json::Value,
///         ctx: &ChannelContext,
///     ) -> Result<Reply, ()> {
///         ctx.broadcaster().broadcast(
///             "chat:lobby",
///             event,
///             payload,
///         );
///         Ok(Reply::None)
///     }
/// }
/// ```
#[async_trait]
pub trait Channel: Send + Sync + 'static {
    /// Called when a client requests to join this topic.
    ///
    /// Return `Ok(reply_payload)` to accept the join. Return
    /// `Err(reason)` to reject.
    async fn join(
        &self,
        topic: &str,
        payload: serde_json::Value,
        ctx: &ChannelContext,
    ) -> Result<serde_json::Value, String>;

    /// Called for each incoming message on a joined topic.
    async fn handle(
        &self,
        event: &str,
        payload: serde_json::Value,
        ctx: &ChannelContext,
    ) -> Result<Reply, ()>;

    /// Called when a client leaves or disconnects from this topic.
    ///
    /// The default implementation is a no-op; override it to release
    /// per-subscriber resources (e.g. presence tracking).
    async fn leave(&self, topic: &str, ctx: &ChannelContext) {
        let _ = (topic, ctx);
    }
}

/// Context available inside [`Channel`] handlers.
///
/// Carries the pieces a handler typically needs: shared application
/// state, the connecting socket's identifier, and the [`Broadcaster`]
/// used to push messages to other subscribers.
pub struct ChannelContext {
    pub(crate) state: Arc<TypeMap>,
    pub(crate) socket_id: String,
    pub(crate) broadcaster: Broadcaster,
}

impl ChannelContext {
    /// Access application state.
    ///
    /// `TypeMap` is crate-internal, so this accessor is used by
    /// framework internals (e.g. the future channel dispatch loop);
    /// channel implementations should use [`ChannelContext::get`] for
    /// typed access to state instead.
    // Unused outside tests until the channel dispatch loop (a later
    // task) needs raw access to the TypeMap — see the
    // ws-channels-workspace task list.
    #[allow(dead_code)]
    pub(crate) fn state(&self) -> &TypeMap {
        &self.state
    }

    /// Get a typed value from application state (same as `State<T>`).
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.state.get_shared::<T>()
    }

    /// The unique identifier for this WebSocket connection.
    pub fn socket_id(&self) -> &str {
        &self.socket_id
    }

    /// The broadcaster for pushing messages to channel subscribers.
    pub fn broadcaster(&self) -> &Broadcaster {
        &self.broadcaster
    }
}

/// Wire format for channel messages.
///
/// Mirrors the Phoenix Channels message envelope: a `topic` the message
/// targets, an `event` name, an arbitrary JSON `payload`, and an
/// optional `ref` used to correlate replies with the request that
/// triggered them.
// Unused outside tests until the channel dispatch loop (a later task)
// deserializes incoming socket frames into this type — see the
// ws-channels-workspace task list.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChannelMessage {
    /// The topic this message targets (e.g., "chat:lobby").
    pub topic: String,
    /// The event name (e.g., "new_msg", "phx_join", "phx_leave").
    pub event: String,
    /// The message payload.
    pub payload: serde_json::Value,
    /// Optional reference for request/reply correlation.
    #[serde(rename = "ref")]
    pub msg_ref: Option<String>,
}

/// A reply from a [`Channel`] handler.
#[derive(Debug)]
pub enum Reply {
    /// Send a JSON payload back to the caller only.
    Ok(serde_json::Value),
    /// Broadcast a message to all subscribers of the current topic.
    Broadcast {
        /// The event name for the broadcast.
        event: String,
        /// The payload to broadcast.
        payload: serde_json::Value,
    },
    /// No reply needed.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::broadcaster::Broadcaster;
    use crate::ws::pubsub::MemoryPubSub;

    struct EchoChannel;

    #[async_trait::async_trait]
    impl Channel for EchoChannel {
        async fn join(
            &self,
            _topic: &str,
            payload: serde_json::Value,
            _ctx: &ChannelContext,
        ) -> Result<serde_json::Value, String> {
            Ok(payload)
        }

        async fn handle(
            &self,
            event: &str,
            payload: serde_json::Value,
            _ctx: &ChannelContext,
        ) -> Result<Reply, ()> {
            Ok(Reply::Ok(serde_json::json!({
                "echo_event": event,
                "echo_payload": payload,
            })))
        }
    }

    fn test_ctx() -> ChannelContext {
        let state = Arc::new(crate::state::TypeMap::new());
        let broadcaster = Broadcaster::new(Arc::new(MemoryPubSub));
        ChannelContext {
            state,
            socket_id: "test-socket-1".into(),
            broadcaster,
        }
    }

    #[tokio::test]
    async fn channel_join_returns_ok() {
        let ch = EchoChannel;
        let ctx = test_ctx();
        let result = ch
            .join("chat:lobby", serde_json::json!({"user": "alice"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["user"], "alice");
    }

    #[tokio::test]
    async fn channel_handle_returns_reply() {
        let ch = EchoChannel;
        let ctx = test_ctx();
        let result = ch.handle("ping", serde_json::json!({"n": 1}), &ctx).await;
        assert!(result.is_ok());
        match result.unwrap() {
            Reply::Ok(v) => {
                assert_eq!(v["echo_event"], "ping");
            }
            _ => panic!("expected Reply::Ok"),
        }
    }

    #[tokio::test]
    async fn channel_leave_default_is_noop() {
        let ch = EchoChannel;
        let ctx = test_ctx();
        ch.leave("chat:lobby", &ctx).await;
        // Should not panic — default leave is a no-op
    }

    #[test]
    fn context_socket_id() {
        let ctx = test_ctx();
        assert_eq!(ctx.socket_id(), "test-socket-1");
    }

    #[test]
    fn channel_message_deserializes() {
        let json = r#"{"topic":"chat:lobby","event":"new_msg","payload":{"text":"hi"},"ref":"1"}"#;
        let msg: ChannelMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.topic, "chat:lobby");
        assert_eq!(msg.event, "new_msg");
        assert_eq!(msg.msg_ref.as_deref(), Some("1"));
    }

    #[test]
    fn channel_message_without_ref() {
        let json = r#"{"topic":"chat:lobby","event":"ping","payload":null}"#;
        let msg: ChannelMessage = serde_json::from_str(json).unwrap();
        assert!(msg.msg_ref.is_none());
    }

    #[test]
    fn channel_message_serializes() {
        let msg = ChannelMessage {
            topic: "chat:lobby".into(),
            event: "phx_reply".into(),
            payload: serde_json::json!({"status": "ok"}),
            msg_ref: Some("1".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"ref\":\"1\""));
    }
}
