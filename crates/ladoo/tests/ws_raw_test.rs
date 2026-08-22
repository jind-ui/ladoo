//! Integration tests for raw WebSocket support.
//!
//! These tests require the `ws` and `test-server` features.
#![cfg(feature = "ws")]

use futures_util::{SinkExt, StreamExt};
use ladoo::prelude::*;
use tokio_tungstenite::tungstenite;

#[tokio::test]
async fn raw_ws_echo() {
    let server = App::test()
        .get(
            "/ws",
            ladoo::ws::websocket(|mut ws: ladoo::ws::WebSocket| async move {
                while let Some(Ok(msg)) = ws.recv().await {
                    ws.send(msg).await.ok();
                }
            }),
        )
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    ws.send(tungstenite::Message::Text("hello".into()))
        .await
        .unwrap();
    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg, tungstenite::Message::Text("hello".into()));

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_invalid_upgrade_returns_400() {
    let server = App::test()
        .get(
            "/ws",
            ladoo::ws::websocket(|mut ws: ladoo::ws::WebSocket| async move {
                while let Some(Ok(msg)) = ws.recv().await {
                    ws.send(msg).await.ok();
                }
            }),
        )
        .spawn()
        .await;

    // Send a normal GET without upgrade headers
    let resp = reqwest::get(format!("http://127.0.0.1:{}/ws", server.port()))
        .await
        .unwrap();
    // Should get a non-101 response (400 or 426)
    assert_ne!(resp.status().as_u16(), 101);
}

#[tokio::test]
async fn ws_middleware_rejects_upgrade() {
    use ladoo::context::Context;
    use ladoo::middleware::{Middleware, Next};
    use std::future::Future;
    use std::pin::Pin;

    struct RejectAll;

    impl Middleware for RejectAll {
        fn call(
            &self,
            _ctx: Context,
            _next: Next,
        ) -> Pin<Box<dyn Future<Output = ladoo::error::Result<Response>> + Send>> {
            Box::pin(async { Err(ladoo::error::Error::unauthorized("nope")) })
        }
    }

    let server = App::test()
        .use_mw(RejectAll)
        .get(
            "/ws",
            ladoo::ws::websocket(|_ws: ladoo::ws::WebSocket| async {}),
        )
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let result = tokio_tungstenite::connect_async(&url).await;
    // Connection should fail — middleware rejected the upgrade
    assert!(result.is_err());
}

#[tokio::test]
async fn ws_binary_message_round_trip() {
    let server = App::test()
        .get(
            "/ws",
            ladoo::ws::websocket(|mut ws: ladoo::ws::WebSocket| async move {
                while let Some(Ok(msg)) = ws.recv().await {
                    ws.send(msg).await.ok();
                }
            }),
        )
        .spawn()
        .await;

    let url = format!("ws://127.0.0.1:{}/ws", server.port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    let payload = vec![0u8, 1, 2, 3, 255];
    ws.send(tungstenite::Message::Binary(payload.clone()))
        .await
        .unwrap();
    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg, tungstenite::Message::Binary(payload));

    ws.close(None).await.ok();
}
