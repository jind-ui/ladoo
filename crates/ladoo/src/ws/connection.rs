//! The per-connection Channel dispatch loop.
//!
//! Every WebSocket connection to the channel endpoint (registered by
//! [`crate::app::App::channel`]) is driven by [`run_channel_loop`], spawned
//! as its own task. It parses incoming frames as [`ChannelMessage`]s,
//! dispatches `phx_join` / `phx_leave` / arbitrary events to the matching
//! [`Channel`], and forwards [`BroadcastEvent`]s the connection is
//! subscribed to back out over the socket.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;

use super::broadcaster::Broadcaster;
use super::channel::{self, ChannelContext, ChannelMessage};
use super::router::ChannelRouter;
use super::socket::{Message, WebSocket};
use crate::state::TypeMap;

/// Run the channel connection loop for one WebSocket client.
///
/// Spawned as a task for each client that connects to the channel
/// endpoint (see [`crate::ws::upgrade::websocket_channel`]). It:
///
/// 1. Generates a UUID v4 `socket_id` and builds a [`ChannelContext`].
/// 2. Subscribes to the [`Broadcaster`]'s local (in-process) channel.
/// 3. Selects between incoming client frames and broadcast events until
///    the connection closes.
/// 4. On disconnect, calls [`Channel::leave`] for every topic the
///    connection had joined.
pub(crate) async fn run_channel_loop(
    mut ws: WebSocket,
    router: Arc<ChannelRouter>,
    broadcaster: Broadcaster,
    state: Arc<TypeMap>,
) {
    let socket_id = uuid::Uuid::new_v4().to_string();
    let ctx = ChannelContext {
        state,
        socket_id: socket_id.clone(),
        broadcaster: broadcaster.clone(),
    };

    broadcaster.ensure_pubsub_forwarder();

    let mut joined_topics: HashSet<String> = HashSet::new();
    let mut broadcast_rx = broadcaster.subscribe_local();

    loop {
        tokio::select! {
            // Incoming WS message from client.
            msg = ws.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let channel_msg: ChannelMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        handle_channel_message(
                            &channel_msg,
                            &mut ws,
                            &router,
                            &ctx,
                            &mut joined_topics,
                        )
                        .await;
                    }
                    Some(Ok(Message::Close)) | None => break,
                    Some(Ok(Message::Binary(_))) => {
                        // Binary frames are ignored — they are not part of
                        // the channel wire protocol.
                    }
                    Some(Err(_)) => break,
                }
            }

            // Broadcast event from the Broadcaster.
            event = broadcast_rx.recv() => {
                match event {
                    Ok(evt) => {
                        // Direct messages (Broadcaster::send_to) use a
                        // synthetic `__direct:<socket_id>` topic instead of
                        // a real channel topic.
                        let is_direct = evt.topic == format!("__direct:{socket_id}");
                        let is_topic = joined_topics.contains(&evt.topic);

                        if !is_direct && !is_topic {
                            continue;
                        }

                        if let Some(ref exclude) = evt.exclude_socket {
                            if exclude == &socket_id {
                                continue;
                            }
                        }

                        let msg = ChannelMessage {
                            topic: evt.topic,
                            event: evt.event,
                            payload: evt.payload,
                            msg_ref: None,
                        };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = ws.send(Message::Text(json)).await;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    // Cleanup: leave all joined topics.
    for topic in &joined_topics {
        if let Some(ch) = router.find(topic) {
            ch.leave(topic, &ctx).await;
        }
    }
}

/// Dispatch one parsed [`ChannelMessage`] from the client: `phx_join`,
/// `phx_leave`, or an arbitrary event forwarded to [`Channel::handle`].
async fn handle_channel_message(
    msg: &ChannelMessage,
    ws: &mut WebSocket,
    router: &ChannelRouter,
    ctx: &ChannelContext,
    joined: &mut HashSet<String>,
) {
    match msg.event.as_str() {
        "phx_join" => {
            let ch = match router.find(&msg.topic) {
                Some(ch) => ch,
                None => {
                    send_reply(
                        ws,
                        &msg.topic,
                        "phx_error",
                        serde_json::json!({"reason": "no such topic"}),
                        msg.msg_ref.clone(),
                    )
                    .await;
                    return;
                }
            };

            match ch.join(&msg.topic, msg.payload.clone(), ctx).await {
                Ok(response) => {
                    joined.insert(msg.topic.clone());
                    send_reply(
                        ws,
                        &msg.topic,
                        "phx_reply",
                        serde_json::json!({"status": "ok", "response": response}),
                        msg.msg_ref.clone(),
                    )
                    .await;
                }
                Err(reason) => {
                    send_reply(
                        ws,
                        &msg.topic,
                        "phx_error",
                        serde_json::json!({"reason": reason}),
                        msg.msg_ref.clone(),
                    )
                    .await;
                }
            }
        }

        "phx_leave" => {
            if joined.remove(&msg.topic) {
                if let Some(ch) = router.find(&msg.topic) {
                    ch.leave(&msg.topic, ctx).await;
                }
                send_reply(
                    ws,
                    &msg.topic,
                    "phx_reply",
                    serde_json::json!({"status": "ok"}),
                    msg.msg_ref.clone(),
                )
                .await;
            }
        }

        event => {
            if !joined.contains(&msg.topic) {
                return;
            }

            let ch = match router.find(&msg.topic) {
                Some(ch) => ch,
                None => return,
            };

            match ch.handle(event, msg.payload.clone(), ctx).await {
                Ok(channel::Reply::Ok(payload)) => {
                    send_reply(
                        ws,
                        &msg.topic,
                        "phx_reply",
                        serde_json::json!({"status": "ok", "response": payload}),
                        msg.msg_ref.clone(),
                    )
                    .await;
                }
                Ok(channel::Reply::Broadcast {
                    event: bcast_event,
                    payload,
                }) => {
                    send_reply(
                        ws,
                        &msg.topic,
                        "phx_reply",
                        serde_json::json!({"status": "ok"}),
                        msg.msg_ref.clone(),
                    )
                    .await;
                    ctx.broadcaster().broadcast_from(
                        &msg.topic,
                        &bcast_event,
                        payload,
                        ctx.socket_id(),
                    );
                }
                Ok(channel::Reply::None) => {}
                Err(()) => {}
            }
        }
    }
}

/// Serialize and send a `ChannelMessage` reply, ignoring send failures
/// (the client may have already disconnected).
async fn send_reply(
    ws: &mut WebSocket,
    topic: &str,
    event: &str,
    payload: serde_json::Value,
    msg_ref: Option<String>,
) {
    let reply = ChannelMessage {
        topic: topic.to_string(),
        event: event.to_string(),
        payload,
        msg_ref,
    };
    if let Ok(json) = serde_json::to_string(&reply) {
        let _ = ws.send(Message::Text(json)).await;
    }
}
