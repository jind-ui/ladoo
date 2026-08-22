//! Broadcaster for delivering messages to WebSocket clients.

use std::sync::Arc;

use super::pubsub::{BroadcastEvent, PubSub};

/// Sends messages to WebSocket channel subscribers from anywhere.
///
/// Available as `State<Broadcaster>` in both HTTP handlers and Channel
/// handlers. Automatically registered when a `ChannelRouter` is
/// configured via `App::channel()`.
///
/// A `Broadcaster` fans a [`BroadcastEvent`] out two ways at once:
///
/// - **Locally**, via an in-process `tokio::sync::broadcast` channel, so
///   every WebSocket connection task on *this* server that has called
///   [`Broadcaster::subscribe_local`] receives it immediately.
/// - **Cross-server**, via the configured [`PubSub`] backend (e.g.
///   Redis), so other server instances can re-deliver it to their own
///   local subscribers.
///
/// `Broadcaster` is cheap to clone — cloning shares the same underlying
/// channel and `PubSub` handle — so it can be stored in application
/// state and handed to every handler.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
/// use ladoo::ws::Broadcaster;
///
/// async fn create_message(
///     req: Request,
///     broadcaster: State<Broadcaster>,
/// ) -> impl IntoResponse {
///     broadcaster.broadcast(
///         "chat:lobby",
///         "new_msg",
///         serde_json::json!({"text": "hello"}),
///     );
///     "ok"
/// }
/// ```
#[derive(Clone)]
pub struct Broadcaster {
    inner: Arc<BroadcasterInner>,
}

struct BroadcasterInner {
    local_tx: tokio::sync::broadcast::Sender<BroadcastEvent>,
    pubsub: Arc<dyn PubSub>,
}

impl Broadcaster {
    /// Create a new Broadcaster with the given PubSub backend.
    // Unused outside tests until the task that wires `App::channel()` /
    // `ChannelRouter` constructs a `Broadcaster` and registers it in app
    // state — see the ws-channels-workspace task list.
    #[allow(dead_code)]
    pub(crate) fn new(pubsub: Arc<dyn PubSub>) -> Self {
        let (local_tx, _) = tokio::sync::broadcast::channel(1024);
        Self {
            inner: Arc::new(BroadcasterInner { local_tx, pubsub }),
        }
    }

    /// Broadcast a message to all subscribers of a topic.
    ///
    /// Delivers to local subscribers synchronously (the send into the
    /// in-process broadcast channel completes before this call returns)
    /// and publishes to the cross-server `PubSub` backend in a spawned
    /// background task, so a slow or failing `PubSub` backend never
    /// blocks the caller.
    pub fn broadcast(&self, topic: &str, event: &str, payload: serde_json::Value) {
        let evt = BroadcastEvent {
            topic: topic.to_string(),
            event: event.to_string(),
            payload,
            exclude_socket: None,
        };
        self.dispatch(evt);
    }

    /// Broadcast to a topic, excluding a specific socket.
    ///
    /// Identical to [`Broadcaster::broadcast`] except the resulting
    /// [`BroadcastEvent::exclude_socket`] is set, so the connection that
    /// triggered the broadcast can skip re-delivering it to itself (no
    /// message echo).
    pub fn broadcast_from(
        &self,
        topic: &str,
        event: &str,
        payload: serde_json::Value,
        exclude: &str,
    ) {
        let evt = BroadcastEvent {
            topic: topic.to_string(),
            event: event.to_string(),
            payload,
            exclude_socket: Some(exclude.to_string()),
        };
        self.dispatch(evt);
    }

    /// Send a message to a specific socket by ID.
    ///
    /// Targets a single connection using a synthetic `__direct:<id>`
    /// topic rather than a real channel topic. Delivered only to local
    /// subscribers — direct messages are not published to the
    /// cross-server `PubSub` backend, since the targeted socket is
    /// assumed to be connected to this server instance.
    pub fn send_to(&self, socket_id: &str, event: &str, payload: serde_json::Value) {
        let evt = BroadcastEvent {
            topic: format!("__direct:{socket_id}"),
            event: event.to_string(),
            payload,
            exclude_socket: None,
        };
        // Direct messages are local-only: no PubSub publish.
        let _ = self.inner.local_tx.send(evt);
    }

    /// Subscribe to the local broadcast channel.
    ///
    /// Used by WS connection tasks to receive broadcast events.
    // Unused outside tests until the channel connection task (a later
    // task) calls this to receive events for its subscribed topics.
    #[allow(dead_code)]
    pub(crate) fn subscribe_local(&self) -> tokio::sync::broadcast::Receiver<BroadcastEvent> {
        self.inner.local_tx.subscribe()
    }

    /// Send an event to local subscribers and publish it to the
    /// cross-server `PubSub` backend in the background.
    fn dispatch(&self, evt: BroadcastEvent) {
        let _ = self.inner.local_tx.send(evt.clone());
        let pubsub = Arc::clone(&self.inner.pubsub);
        tokio::spawn(async move {
            if let Err(_e) = pubsub.publish(evt).await {
                #[cfg(feature = "logging")]
                tracing::warn!(error = %_e, "PubSub publish failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::pubsub::MemoryPubSub;

    #[tokio::test]
    async fn broadcast_delivers_to_local_subscriber() {
        let broadcaster = Broadcaster::new(Arc::new(MemoryPubSub));
        let mut rx = broadcaster.subscribe_local();

        broadcaster.broadcast("chat:lobby", "new_msg", serde_json::json!({"text": "hello"}));

        let event = rx.recv().await.unwrap();
        assert_eq!(event.topic, "chat:lobby");
        assert_eq!(event.event, "new_msg");
        assert!(event.exclude_socket.is_none());
    }

    #[tokio::test]
    async fn broadcast_from_excludes_sender() {
        let broadcaster = Broadcaster::new(Arc::new(MemoryPubSub));
        let mut rx = broadcaster.subscribe_local();

        broadcaster.broadcast_from(
            "chat:lobby",
            "new_msg",
            serde_json::json!({"text": "hello"}),
            "socket-1",
        );

        let event = rx.recv().await.unwrap();
        assert_eq!(event.exclude_socket.as_deref(), Some("socket-1"));
    }

    #[tokio::test]
    async fn send_to_delivers_targeted_event() {
        let broadcaster = Broadcaster::new(Arc::new(MemoryPubSub));
        let mut rx = broadcaster.subscribe_local();

        broadcaster.send_to("socket-42", "direct_msg", serde_json::json!({"text": "hey"}));

        let event = rx.recv().await.unwrap();
        // send_to uses a special topic like "__direct:socket-42"
        assert!(event.topic.contains("socket-42"));
        assert_eq!(event.event, "direct_msg");
    }

    #[tokio::test]
    async fn multiple_subscribers_all_receive() {
        let broadcaster = Broadcaster::new(Arc::new(MemoryPubSub));
        let mut rx1 = broadcaster.subscribe_local();
        let mut rx2 = broadcaster.subscribe_local();

        broadcaster.broadcast("chat:lobby", "ping", serde_json::json!(null));

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.topic, "chat:lobby");
        assert_eq!(e2.topic, "chat:lobby");
    }

    #[test]
    fn broadcaster_is_clone_send_sync() {
        fn assert_clone_send_sync<T: Clone + Send + Sync>() {}
        assert_clone_send_sync::<Broadcaster>();
    }
}
