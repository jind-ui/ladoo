//! WebSocket error types.

/// WebSocket errors.
///
/// Wraps all failure modes that can occur while establishing, running, or
/// broadcasting over a WebSocket connection or Channel — protocol violations,
/// unexpected disconnects, (de)serialization failures, pub/sub backend
/// errors, and invalid upgrade requests.
///
/// # Examples
///
/// ```
/// use ladoo::ws::WsError;
///
/// let err = WsError::ConnectionClosed;
/// assert_eq!(err.to_string(), "WebSocket connection closed");
/// ```
#[derive(Debug)]
pub enum WsError {
    /// WebSocket protocol error (malformed frame, unexpected close).
    Protocol(String),
    /// The connection was closed by the remote peer.
    ConnectionClosed,
    /// Failed to serialize/deserialize a channel message.
    Serialization(String),
    /// PubSub backend error (Redis connection failure, etc.).
    PubSub(String),
    /// The upgrade request was invalid (missing headers, wrong method).
    InvalidUpgrade(String),
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(msg) => write!(f, "WebSocket protocol error: {msg}"),
            Self::ConnectionClosed => write!(f, "WebSocket connection closed"),
            Self::Serialization(msg) => {
                write!(f, "WebSocket serialization error: {msg}")
            }
            Self::PubSub(msg) => write!(f, "WebSocket pub/sub error: {msg}"),
            Self::InvalidUpgrade(msg) => {
                write!(f, "invalid WebSocket upgrade: {msg}")
            }
        }
    }
}

impl std::error::Error for WsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_protocol_error() {
        let err = WsError::Protocol("bad frame".into());
        assert_eq!(err.to_string(), "WebSocket protocol error: bad frame");
    }

    #[test]
    fn display_connection_closed() {
        let err = WsError::ConnectionClosed;
        assert_eq!(err.to_string(), "WebSocket connection closed");
    }

    #[test]
    fn display_serialization_error() {
        let err = WsError::Serialization("invalid json".into());
        assert_eq!(
            err.to_string(),
            "WebSocket serialization error: invalid json"
        );
    }

    #[test]
    fn display_pubsub_error() {
        let err = WsError::PubSub("redis down".into());
        assert_eq!(err.to_string(), "WebSocket pub/sub error: redis down");
    }

    #[test]
    fn display_invalid_upgrade() {
        let err = WsError::InvalidUpgrade("missing header".into());
        assert_eq!(err.to_string(), "invalid WebSocket upgrade: missing header");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WsError>();
    }

    #[test]
    fn error_is_std_error() {
        fn assert_error<T: std::error::Error>() {}
        assert_error::<WsError>();
    }
}
