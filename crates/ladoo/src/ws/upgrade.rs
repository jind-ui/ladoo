//! HTTP-to-WebSocket upgrade detection and response building.

use bytes::Bytes;
use http::HeaderMap;
use http_body_util::Full;

/// Check if headers indicate a valid WebSocket upgrade request.
///
/// A valid upgrade request has a `Connection` header containing the token
/// `upgrade` (case-insensitively) and an `Upgrade` header equal to
/// `websocket` (case-insensitively).
pub(crate) fn check_ws_headers(headers: &HeaderMap) -> bool {
    let has_connection = headers
        .get(http::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);

    let has_upgrade = headers
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    has_connection && has_upgrade
}

/// Returns true if the request is a valid WebSocket upgrade.
///
/// Unused until the router (a later task) wires in upgrade dispatch.
#[allow(dead_code)]
pub(crate) fn is_websocket_upgrade(req: &hyper::Request<hyper::body::Incoming>) -> bool {
    check_ws_headers(req.headers())
}

/// Build the 101 Switching Protocols response for a valid upgrade.
///
/// Computes `Sec-WebSocket-Accept` from the client's `Sec-WebSocket-Key`
/// per RFC 6455 Section 4.2.2.
///
/// Unused until the router (a later task) wires in upgrade dispatch.
#[allow(dead_code)]
pub(crate) fn build_upgrade_response(key: &[u8]) -> hyper::Response<Full<Bytes>> {
    use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

    let accept = derive_accept_key(key);

    hyper::Response::builder()
        .status(http::StatusCode::SWITCHING_PROTOCOLS)
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::CONNECTION, "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_upgrade_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONNECTION, "Upgrade".parse().unwrap());
        headers.insert(http::header::UPGRADE, "websocket".parse().unwrap());
        headers.insert(http::header::SEC_WEBSOCKET_VERSION, "13".parse().unwrap());
        headers.insert(
            "sec-websocket-key",
            "dGhlIHNhbXBsZSBub25jZQ==".parse().unwrap(),
        );
        assert!(check_ws_headers(&headers));
    }

    #[test]
    fn missing_upgrade_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONNECTION, "Upgrade".parse().unwrap());
        // No Upgrade: websocket header
        assert!(!check_ws_headers(&headers));
    }

    #[test]
    fn missing_connection_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::UPGRADE, "websocket".parse().unwrap());
        // No Connection: Upgrade header
        assert!(!check_ws_headers(&headers));
    }

    #[test]
    fn wrong_upgrade_value() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONNECTION, "Upgrade".parse().unwrap());
        headers.insert(http::header::UPGRADE, "h2c".parse().unwrap());
        assert!(!check_ws_headers(&headers));
    }

    #[test]
    fn case_insensitive_upgrade() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONNECTION, "upgrade".parse().unwrap());
        headers.insert(http::header::UPGRADE, "WebSocket".parse().unwrap());
        headers.insert(http::header::SEC_WEBSOCKET_VERSION, "13".parse().unwrap());
        headers.insert(
            "sec-websocket-key",
            "dGhlIHNhbXBsZSBub25jZQ==".parse().unwrap(),
        );
        assert!(check_ws_headers(&headers));
    }

    #[test]
    fn upgrade_response_has_correct_status() {
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        let resp = build_upgrade_response(key);
        assert_eq!(resp.status(), http::StatusCode::SWITCHING_PROTOCOLS);
    }

    #[test]
    fn upgrade_response_has_upgrade_header() {
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        let resp = build_upgrade_response(key);
        assert_eq!(
            resp.headers().get(http::header::UPGRADE).unwrap(),
            "websocket"
        );
    }

    #[test]
    fn upgrade_response_has_connection_header() {
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        let resp = build_upgrade_response(key);
        assert_eq!(
            resp.headers().get(http::header::CONNECTION).unwrap(),
            "Upgrade"
        );
    }

    #[test]
    fn upgrade_response_rfc6455_accept_key() {
        // RFC 6455 example: key "dGhlIHNhbXBsZSBub25jZQ=="
        // Expected accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        let resp = build_upgrade_response(key);
        let accept = resp
            .headers()
            .get("sec-websocket-accept")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
