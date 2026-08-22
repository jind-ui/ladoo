//! Redis PubSub integration test.
//!
//! Skipped unless `REDIS_URL` is set and the `ws-redis` and `test-server`
//! features are enabled.
#![cfg(feature = "ws-redis")]

use futures_util::{SinkExt, StreamExt};
use ladoo::prelude::*;
use ladoo::ws::{Channel, ChannelContext, ChannelRouter, RedisPubSub, Reply};
use tokio_tungstenite::tungstenite;

struct EchoChannel;

#[async_trait]
impl Channel for EchoChannel {
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

#[tokio::test]
async fn cross_server_broadcast_via_redis() {
    let redis_url = match std::env::var("REDIS_URL") {
        Ok(u) => u,
        Err(_) => return, // Skip without Redis
    };

    // Server 1
    let pubsub1 = RedisPubSub::new(&redis_url)
        .unwrap()
        .channel("ladoo:test:cross");
    let server1 = App::test()
        .channel_with_pubsub(ChannelRouter::new().route("chat:*", EchoChannel), pubsub1)
        .spawn()
        .await;

    // Server 2
    let pubsub2 = RedisPubSub::new(&redis_url)
        .unwrap()
        .channel("ladoo:test:cross");
    let server2 = App::test()
        .channel_with_pubsub(ChannelRouter::new().route("chat:*", EchoChannel), pubsub2)
        .spawn()
        .await;

    // Client on server 1
    let url1 = format!("ws://127.0.0.1:{}/ws", server1.port());
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url1).await.unwrap();

    // Client on server 2
    let url2 = format!("ws://127.0.0.1:{}/ws", server2.port());
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url2).await.unwrap();

    // Both join chat:lobby
    let join_msg = serde_json::json!({
        "topic": "chat:lobby",
        "event": "phx_join",
        "payload": {},
        "ref": "1"
    })
    .to_string();

    ws1.send(tungstenite::Message::Text(join_msg.clone().into()))
        .await
        .unwrap();
    let _ = ws1.next().await; // join reply

    ws2.send(tungstenite::Message::Text(join_msg.into()))
        .await
        .unwrap();
    let _ = ws2.next().await; // join reply

    // Allow Redis subscription to establish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Client 1 sends a message
    let msg = serde_json::json!({
        "topic": "chat:lobby",
        "event": "new_msg",
        "payload": {"text": "cross-server"},
        "ref": "2"
    })
    .to_string();

    ws1.send(tungstenite::Message::Text(msg.into()))
        .await
        .unwrap();

    // Client 2 (on different server) should receive the broadcast
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), ws2.next())
        .await
        .expect("ws2 should receive cross-server broadcast")
        .unwrap()
        .unwrap();

    let text = received.into_text().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["event"], "new_msg");
    assert_eq!(parsed["payload"]["text"], "cross-server");

    ws1.close(None).await.ok();
    ws2.close(None).await.ok();
}
