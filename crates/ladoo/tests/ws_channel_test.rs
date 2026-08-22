//! Integration tests for WebSocket Channels: `App::channel()`, the
//! per-connection channel loop, and broadcast delivery.
//!
//! These tests require the `ws` and `test-server` features.
#![cfg(feature = "ws")]

use futures_util::{SinkExt, StreamExt};
use ladoo::prelude::*;
use ladoo::ws::{Channel, ChannelContext, ChannelRouter, Reply};
use tokio_tungstenite::tungstenite;

struct ChatChannel;

#[async_trait]
impl Channel for ChatChannel {
    async fn join(
        &self,
        topic: &str,
        _payload: serde_json::Value,
        _ctx: &ChannelContext,
    ) -> std::result::Result<serde_json::Value, String> {
        Ok(serde_json::json!({"joined": topic}))
    }

    async fn handle(
        &self,
        event: &str,
        payload: serde_json::Value,
        _ctx: &ChannelContext,
    ) -> std::result::Result<Reply, ()> {
        Ok(Reply::Broadcast {
            event: event.to_string(),
            payload,
        })
    }
}

fn join_msg(topic: &str) -> String {
    serde_json::json!({
        "topic": topic,
        "event": "phx_join",
        "payload": {},
        "ref": "1"
    })
    .to_string()
}

fn leave_msg(topic: &str) -> String {
    serde_json::json!({
        "topic": topic,
        "event": "phx_leave",
        "payload": {},
        "ref": "2"
    })
    .to_string()
}

fn channel_msg(topic: &str, event: &str, payload: serde_json::Value) -> String {
    serde_json::json!({
        "topic": topic,
        "event": event,
        "payload": payload,
        "ref": "3"
    })
    .to_string()
}

#[tokio::test]
async fn channel_join_and_reply() {
    let server = App::test()
        .channel(ChannelRouter::new().route("chat:*", ChatChannel))
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    // Join chat:lobby
    ws.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();

    // Should receive phx_reply with status ok
    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(reply["event"], "phx_reply");
    assert_eq!(reply["ref"], "1");

    ws.close(None).await.ok();
}

#[tokio::test]
async fn channel_messaging_broadcast() {
    let server = App::test()
        .channel(ChannelRouter::new().route("chat:*", ChatChannel))
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());

    // Connect two clients
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS1 connect failed");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS2 connect failed");

    // Both join chat:lobby
    ws1.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();
    let _ = ws1.next().await; // consume join reply

    ws2.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();
    let _ = ws2.next().await; // consume join reply

    // ws1 sends a message
    ws1.send(tungstenite::Message::Text(
        channel_msg("chat:lobby", "new_msg", serde_json::json!({"text": "hello"})).into(),
    ))
    .await
    .unwrap();

    // ws1 gets the phx_reply
    let _reply1 = ws1.next().await.unwrap().unwrap();
    // May also get the broadcast

    // ws2 should receive the broadcast
    let msg2 = tokio::time::timeout(std::time::Duration::from_secs(2), ws2.next())
        .await
        .expect("ws2 should receive broadcast")
        .unwrap()
        .unwrap();
    let text2 = msg2.into_text().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text2).unwrap();
    assert_eq!(parsed["event"], "new_msg");
    assert_eq!(parsed["payload"]["text"], "hello");

    ws1.close(None).await.ok();
    ws2.close(None).await.ok();
}

#[tokio::test]
async fn http_to_ws_broadcast() {
    use ladoo::ws::Broadcaster;

    async fn push_handler(broadcaster: State<Broadcaster>) -> &'static str {
        broadcaster.broadcast(
            "chat:lobby",
            "server_push",
            serde_json::json!({"from": "http"}),
        );
        "ok"
    }

    let server = App::test()
        .channel(ChannelRouter::new().route("chat:*", ChatChannel))
        .post("/push", push_handler)
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    // Join chat:lobby
    ws.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();
    let _ = ws.next().await; // consume join reply

    // HTTP POST to trigger broadcast
    let client = reqwest::Client::new();
    client
        .post(format!("http://127.0.0.1:{}/push", server.port()))
        .send()
        .await
        .unwrap();

    // WS client should receive the broadcast
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("should receive HTTP-pushed broadcast")
        .unwrap()
        .unwrap();
    let text = msg.into_text().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["event"], "server_push");
    assert_eq!(parsed["payload"]["from"], "http");

    ws.close(None).await.ok();
}

#[tokio::test]
async fn channel_join_rejected() {
    struct RejectChannel;

    #[async_trait]
    impl Channel for RejectChannel {
        async fn join(
            &self,
            _topic: &str,
            _payload: serde_json::Value,
            _ctx: &ChannelContext,
        ) -> std::result::Result<serde_json::Value, String> {
            Err("not allowed".into())
        }

        async fn handle(
            &self,
            _event: &str,
            _payload: serde_json::Value,
            _ctx: &ChannelContext,
        ) -> std::result::Result<Reply, ()> {
            Ok(Reply::None)
        }
    }

    let server = App::test()
        .channel(ChannelRouter::new().route("secret:*", RejectChannel))
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    ws.send(tungstenite::Message::Text(join_msg("secret:room").into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(reply["event"], "phx_error");

    ws.close(None).await.ok();
}

#[tokio::test]
async fn channel_leave_stops_broadcast_delivery() {
    let server = App::test()
        .channel(ChannelRouter::new().route("chat:*", ChatChannel))
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS1 connect failed");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS2 connect failed");

    ws1.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();
    let _ = ws1.next().await;

    ws2.send(tungstenite::Message::Text(join_msg("chat:lobby").into()))
        .await
        .unwrap();
    let _ = ws2.next().await;

    // ws2 leaves
    ws2.send(tungstenite::Message::Text(leave_msg("chat:lobby").into()))
        .await
        .unwrap();
    let leave_reply = ws2.next().await.unwrap().unwrap();
    let leave_text = leave_reply.into_text().unwrap();
    let leave_parsed: serde_json::Value = serde_json::from_str(&leave_text).unwrap();
    assert_eq!(leave_parsed["event"], "phx_reply");

    // ws1 sends a message; ws2 should NOT receive it since it left
    ws1.send(tungstenite::Message::Text(
        channel_msg("chat:lobby", "new_msg", serde_json::json!({"text": "hi"})).into(),
    ))
    .await
    .unwrap();
    let _ = ws1.next().await; // consume ws1's own phx_reply

    let result = tokio::time::timeout(std::time::Duration::from_millis(500), ws2.next()).await;
    assert!(
        result.is_err(),
        "ws2 should not receive broadcast after leaving"
    );

    ws1.close(None).await.ok();
    ws2.close(None).await.ok();
}
