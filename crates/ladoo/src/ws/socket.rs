//! WebSocket connection type.

use futures_util::{SinkExt, StreamExt};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;
use tokio_tungstenite::WebSocketStream;

use super::error::WsError;

/// A WebSocket connection.
///
/// Wraps a raw upgraded TCP stream with WebSocket framing. Created by
/// the `websocket()` handler helper or internally by the channel system.
///
/// # Examples
///
/// ```rust,ignore
/// // Echo server
/// while let Some(Ok(msg)) = ws.recv().await {
///     ws.send(msg).await.ok();
/// }
/// ```
pub struct WebSocket {
    inner: WebSocketStream<TokioIo<Upgraded>>,
}

/// A message received from or sent to a WebSocket.
///
/// # Examples
///
/// ```
/// use ladoo::ws::Message;
///
/// let msg = Message::Text("hello".to_string());
/// assert!(matches!(msg, Message::Text(ref s) if s == "hello"));
/// ```
#[derive(Debug, Clone)]
pub enum Message {
    /// UTF-8 text frame.
    Text(String),
    /// Binary data frame.
    Binary(Vec<u8>),
    /// Connection close frame.
    Close,
}

impl WebSocket {
    /// Create a WebSocket from an upgraded hyper connection.
    ///
    /// Unused until the upgrade handler (a later task) wires the hyper
    /// `Upgraded` connection into this type.
    #[allow(dead_code)]
    pub(crate) fn from_upgraded(stream: WebSocketStream<TokioIo<Upgraded>>) -> Self {
        Self { inner: stream }
    }

    /// Receive the next message, or None if the connection closed.
    pub async fn recv(&mut self) -> Option<Result<Message, WsError>> {
        loop {
            match self.inner.next().await {
                Some(Ok(raw)) => {
                    if let Some(msg) = Message::from_tungstenite(raw) {
                        return Some(Ok(msg));
                    }
                    // Ping/Pong are handled automatically by tungstenite;
                    // skip them and read the next frame.
                }
                Some(Err(e)) => {
                    return Some(Err(WsError::Protocol(e.to_string())));
                }
                None => return None,
            }
        }
    }

    /// Send a message to the remote peer.
    pub async fn send(&mut self, msg: Message) -> Result<(), WsError> {
        self.inner
            .send(msg.into_tungstenite())
            .await
            .map_err(|e| WsError::Protocol(e.to_string()))
    }

    /// Initiate a graceful close handshake.
    pub async fn close(&mut self) -> Result<(), WsError> {
        self.inner
            .close(None)
            .await
            .map_err(|e| WsError::Protocol(e.to_string()))
    }
}

impl Message {
    /// Convert from tungstenite's message type.
    ///
    /// Returns `None` for Ping/Pong (handled automatically by tungstenite).
    pub(crate) fn from_tungstenite(msg: TungsteniteMessage) -> Option<Self> {
        match msg {
            TungsteniteMessage::Text(s) => Some(Self::Text(s)),
            TungsteniteMessage::Binary(b) => Some(Self::Binary(b)),
            TungsteniteMessage::Close(_) => Some(Self::Close),
            TungsteniteMessage::Ping(_) | TungsteniteMessage::Pong(_) => None,
            _ => None,
        }
    }

    /// Convert into tungstenite's message type.
    pub(crate) fn into_tungstenite(self) -> TungsteniteMessage {
        match self {
            Self::Text(s) => TungsteniteMessage::Text(s),
            Self::Binary(b) => TungsteniteMessage::Binary(b),
            Self::Close => TungsteniteMessage::Close(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;

    #[test]
    fn from_tungstenite_text() {
        let raw = TungsteniteMessage::Text("hello".into());
        let msg = Message::from_tungstenite(raw);
        assert!(matches!(msg, Some(Message::Text(ref s)) if s == "hello"));
    }

    #[test]
    fn from_tungstenite_binary() {
        let raw = TungsteniteMessage::Binary(vec![1, 2, 3]);
        let msg = Message::from_tungstenite(raw);
        assert!(matches!(msg, Some(Message::Binary(ref b)) if b == &[1, 2, 3]));
    }

    #[test]
    fn from_tungstenite_close() {
        let raw = TungsteniteMessage::Close(None);
        let msg = Message::from_tungstenite(raw);
        assert!(matches!(msg, Some(Message::Close)));
    }

    #[test]
    fn from_tungstenite_ping_ignored() {
        let raw = TungsteniteMessage::Ping(vec![]);
        let msg = Message::from_tungstenite(raw);
        assert!(msg.is_none());
    }

    #[test]
    fn from_tungstenite_pong_ignored() {
        let raw = TungsteniteMessage::Pong(vec![]);
        let msg = Message::from_tungstenite(raw);
        assert!(msg.is_none());
    }

    #[test]
    fn into_tungstenite_text() {
        let msg = Message::Text("hello".into());
        let raw = msg.into_tungstenite();
        assert!(matches!(raw, TungsteniteMessage::Text(ref s) if s == "hello"));
    }

    #[test]
    fn into_tungstenite_binary() {
        let msg = Message::Binary(vec![1, 2, 3]);
        let raw = msg.into_tungstenite();
        assert!(matches!(raw, TungsteniteMessage::Binary(ref b) if b == &[1, 2, 3]));
    }

    #[test]
    fn into_tungstenite_close() {
        let msg = Message::Close;
        let raw = msg.into_tungstenite();
        assert!(matches!(raw, TungsteniteMessage::Close(None)));
    }

    #[test]
    fn message_types_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Message>();
        assert_send::<WebSocket>();
    }
}
