//! HTTP-to-WebSocket upgrade detection, response building, and the
//! [`websocket()`] handler helper.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::HeaderMap;
use http_body_util::Full;

use crate::handler::Handler;
use crate::request::Request;
use crate::response::Response;
use crate::state::TypeMap;

use super::broadcaster::Broadcaster;
use super::router::ChannelRouter;
use super::socket::WebSocket;

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
/// Checked by [`crate::server::handle_request`] before the request body is
/// collected, since a WS upgrade needs the raw `Incoming` body stream.
pub(crate) fn is_websocket_upgrade(req: &hyper::Request<hyper::body::Incoming>) -> bool {
    check_ws_headers(req.headers())
}

/// Compute `Sec-WebSocket-Accept` from the client's `Sec-WebSocket-Key`
/// per RFC 6455 Section 4.2.2.
fn derive_ws_accept(key: &[u8]) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key)
}

/// Build the 101 Switching Protocols response for a valid upgrade.
///
/// Unused in production code paths (the [`websocket()`] handler builds its
/// own framework [`Response`] so it can be routed through middleware like
/// any other handler), but kept as a standalone, independently-testable
/// unit for the handshake response shape.
#[allow(dead_code)]
pub(crate) fn build_upgrade_response(key: &[u8]) -> hyper::Response<Full<Bytes>> {
    let accept = derive_ws_accept(key);

    hyper::Response::builder()
        .status(http::StatusCode::SWITCHING_PROTOCOLS)
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::CONNECTION, "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

/// Internal handle passed through per-request state during a WebSocket
/// upgrade.
///
/// Holds the hyper `OnUpgrade` future (so the [`websocket()`] handler can
/// await the raw upgraded connection once the 101 response has been
/// accepted by the middleware chain) and the client's `Sec-WebSocket-Key`
/// (used to compute the `Sec-WebSocket-Accept` response header). The
/// `Mutex` wrapper makes this `Sync` — required because `Request::provide`
/// stores values in a `TypeMap` keyed by `Send + Sync + 'static` types —
/// even though only one handler ever takes the `OnUpgrade` out of it.
pub(crate) struct WsUpgradeHandle {
    pub(crate) on_upgrade: Mutex<Option<hyper::upgrade::OnUpgrade>>,
    pub(crate) ws_key: Vec<u8>,
}

/// Wrap a handler function as a WebSocket endpoint.
///
/// The returned [`Handler`] is registered like any other route handler
/// (e.g. via `App::get`). When an incoming request is a valid WebSocket
/// upgrade, the server routes it through the middleware chain first —
/// middleware can reject the upgrade (e.g. authentication) just like a
/// normal request — and if it's accepted, this handler completes the
/// handshake and spawns `handler` to run for the lifetime of the
/// connection.
///
/// If this handler is reached *without* a WebSocket upgrade in progress
/// (for example, a plain `GET` request to a route registered with
/// `websocket()`), it returns `426 Upgrade Required`.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
/// use ladoo::ws::{websocket, WebSocket};
///
/// App::new()
///     .get("/ws", websocket(|mut ws: WebSocket| async move {
///         while let Some(Ok(msg)) = ws.recv().await {
///             ws.send(msg).await.ok();
///         }
///     }));
/// ```
pub fn websocket<F, Fut>(handler: F) -> Box<dyn Handler>
where
    F: Fn(WebSocket) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Box::new(WsHandlerImpl {
        f: Arc::new(handler),
    })
}

struct WsHandlerImpl<F> {
    f: Arc<F>,
}

impl<F, Fut> Handler for WsHandlerImpl<F>
where
    F: Fn(WebSocket) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn call(&self, req: Request) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
        // Clone the Arc *now*, while we still have `&self` — the spawned
        // upgrade task below needs a 'static, owned copy of the handler.
        let f = Arc::clone(&self.f);

        Box::pin(async move {
            // `handle_ws_upgrade` (server.rs) stashes the OnUpgrade handle
            // in per-request state before routing through middleware. If
            // it's missing, this handler was reached without a WS upgrade
            // in flight — e.g. a plain GET to a `websocket()` route.
            let handle = match req.per_request().get_shared::<WsUpgradeHandle>() {
                Some(h) => h,
                None => {
                    return Response::new(
                        http::StatusCode::UPGRADE_REQUIRED,
                        HeaderMap::new(),
                        Bytes::from_static(b"WebSocket upgrade required"),
                    );
                }
            };

            let on_upgrade = handle
                .on_upgrade
                .lock()
                .expect("WsUpgradeHandle mutex poisoned")
                .take();

            let Some(on_upgrade) = on_upgrade else {
                // Already taken — the route matched twice somehow, or a
                // retry. Either way there's no upgrade left to complete.
                return Response::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    HeaderMap::new(),
                    Bytes::from_static(b"WebSocket upgrade already completed"),
                );
            };

            let accept = derive_ws_accept(&handle.ws_key);

            // Spawn the connection handler now, before returning the 101.
            // It won't actually run the user's `f` until `on_upgrade`
            // resolves, which only happens once this response has been
            // sent and hyper completes the upgrade on the underlying
            // connection.
            tokio::spawn(async move {
                match on_upgrade.await {
                    Ok(upgraded) => {
                        let ws_stream = tokio_tungstenite::WebSocketStream::from_raw_socket(
                            hyper_util::rt::TokioIo::new(upgraded),
                            tokio_tungstenite::tungstenite::protocol::Role::Server,
                            None,
                        )
                        .await;
                        let ws = WebSocket::from_upgraded(ws_stream);
                        f(ws).await;
                    }
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::warn!("WebSocket upgrade failed: {e}");
                        #[cfg(not(feature = "logging"))]
                        let _ = e;
                    }
                }
            });

            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::UPGRADE,
                http::HeaderValue::from_static("websocket"),
            );
            headers.insert(
                http::header::CONNECTION,
                http::HeaderValue::from_static("Upgrade"),
            );
            headers.insert(
                http::HeaderName::from_static("sec-websocket-accept"),
                http::HeaderValue::from_str(&accept)
                    .expect("derive_accept_key always produces a valid header value"),
            );

            Response::new(http::StatusCode::SWITCHING_PROTOCOLS, headers, Bytes::new())
        })
    }
}

/// Create the WebSocket handler for the Channel system's `/ws` endpoint.
///
/// Registered by [`crate::app::App::channel`]. Unlike [`websocket()`],
/// which hands the raw [`WebSocket`] to a user-supplied closure, this
/// handler runs the framework's own [`super::connection::run_channel_loop`]
/// for every connection — extracting the [`ChannelRouter`] from the
/// request's application state and pairing it with the given
/// [`Broadcaster`].
pub(crate) fn websocket_channel(broadcaster: Broadcaster) -> Box<dyn Handler> {
    Box::new(ChannelWsHandler { broadcaster })
}

struct ChannelWsHandler {
    broadcaster: Broadcaster,
}

impl Handler for ChannelWsHandler {
    fn call(&self, req: Request) -> Pin<Box<dyn Future<Output = Response> + Send + '_>> {
        let broadcaster = self.broadcaster.clone();
        // The app state is shared (`Arc<TypeMap>`) — clone the handle now,
        // while we still have `&Request`, for use in the spawned task.
        let state: Arc<TypeMap> = req.extensions_arc();

        Box::pin(async move {
            let router = match state.get_shared::<ChannelRouter>() {
                Some(r) => r,
                None => {
                    return Response::new(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        HeaderMap::new(),
                        Bytes::from_static(b"channel router not configured"),
                    );
                }
            };

            let handle = match req.per_request().get_shared::<WsUpgradeHandle>() {
                Some(h) => h,
                None => {
                    return Response::new(
                        http::StatusCode::UPGRADE_REQUIRED,
                        HeaderMap::new(),
                        Bytes::from_static(b"WebSocket upgrade required"),
                    );
                }
            };

            let on_upgrade = handle
                .on_upgrade
                .lock()
                .expect("WsUpgradeHandle mutex poisoned")
                .take();

            let Some(on_upgrade) = on_upgrade else {
                return Response::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    HeaderMap::new(),
                    Bytes::from_static(b"WebSocket upgrade already completed"),
                );
            };

            let accept = derive_ws_accept(&handle.ws_key);

            tokio::spawn(async move {
                match on_upgrade.await {
                    Ok(upgraded) => {
                        let ws_stream = tokio_tungstenite::WebSocketStream::from_raw_socket(
                            hyper_util::rt::TokioIo::new(upgraded),
                            tokio_tungstenite::tungstenite::protocol::Role::Server,
                            None,
                        )
                        .await;
                        let ws = WebSocket::from_upgraded(ws_stream);
                        super::connection::run_channel_loop(ws, router, broadcaster, state).await;
                    }
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::warn!("WebSocket upgrade failed: {e}");
                        #[cfg(not(feature = "logging"))]
                        let _ = e;
                    }
                }
            });

            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::UPGRADE,
                http::HeaderValue::from_static("websocket"),
            );
            headers.insert(
                http::header::CONNECTION,
                http::HeaderValue::from_static("Upgrade"),
            );
            headers.insert(
                http::HeaderName::from_static("sec-websocket-accept"),
                http::HeaderValue::from_str(&accept)
                    .expect("derive_accept_key always produces a valid header value"),
            );

            Response::new(http::StatusCode::SWITCHING_PROTOCOLS, headers, Bytes::new())
        })
    }
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
