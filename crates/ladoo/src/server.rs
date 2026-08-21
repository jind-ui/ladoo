//! HTTP server powered by Hyper.
//!
//! The server converts incoming hyper requests to [`Request`], routes them
//! through the [`Router`], calls the matched [`Handler`], and sends the
//! [`Response`] back. Unmatched routes receive a 404 Not Found.
//!
//! # Entry Points
//!
//! - [`App::run`] — Blocking. Creates a Tokio runtime and starts the server.
//!   Use this for simple `fn main()` apps.
//! - [`App::serve_listener`] — Async. Takes a pre-bound `TcpListener`.
//!   Use this in tests or when managing your own runtime.
//!
//! [`App::run`]: crate::app::App::run
//! [`App::serve_listener`]: crate::app::App::serve_listener
//! [`Handler`]: crate::handler::Handler
//! [`Response`]: crate::response::Response

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::middleware::Middleware;
use crate::plugin::ShutdownHook;
use crate::request::Request;
use crate::router::Router;
use crate::state::TypeMap;

#[cfg(feature = "tls")]
use tokio_rustls::TlsAcceptor;

/// Start serving HTTP requests using the given router, listener,
/// application state, and global middleware stack.
///
/// This is the core server loop. It accepts connections, routes requests,
/// and sends responses. Each connection is handled in a separate Tokio task.
/// `state` is shared across every request and is what powers the
/// [`State<T>`](crate::state::State) extractor. `global_middleware` runs on
/// every matched route, ahead of any route-specific middleware.
///
/// `shutdown` resolves when the server should stop accepting new
/// connections. Once it resolves, in-flight connections are given
/// `shutdown_timeout` to finish gracefully — hyper stops accepting new
/// requests on each connection but lets the current one complete — before
/// being forcibly aborted.
///
/// `body_limit` caps the size (in bytes) of each request body; requests
/// exceeding it receive a `413 Payload Too Large` response before the
/// handler runs.
///
/// When the `tls` feature is enabled, `tls_acceptor` optionally wraps
/// every accepted connection in a TLS handshake (10-second timeout)
/// before serving HTTP over it. `None` serves plain HTTP; `Some` serves
/// HTTPS exclusively on this listener.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    router: Router,
    listener: TcpListener,
    state: Arc<TypeMap>,
    global_middleware: Vec<Arc<dyn Middleware>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    shutdown_timeout: Duration,
    shutdown_hooks: Vec<ShutdownHook>,
    body_limit: usize,
    #[cfg(feature = "tls")] tls_acceptor: Option<TlsAcceptor>,
) {
    let router = Arc::new(router);
    let global_middleware: Arc<[Arc<dyn Middleware>]> = global_middleware.into();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut connections = JoinSet::new();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, addr) = match result {
                    Ok(conn) => conn,
                    Err(_) => continue,
                };

                let router = router.clone();
                let state = state.clone();
                let global_mw = global_middleware.clone();
                let rx = shutdown_rx.clone();
                #[cfg(feature = "tls")]
                let tls_acceptor = tls_acceptor.clone();

                connections.spawn(async move {
                    let router = router.clone();
                    let state = state.clone();
                    let global_mw = global_mw.clone();
                    let addr = addr;

                    let service = service_fn(move |hyper_req: hyper::Request<hyper::body::Incoming>| {
                        let router = router.clone();
                        let state = state.clone();
                        let global_mw = global_mw.clone();
                        async move {
                            let response =
                                handle_request(&router, hyper_req, state, &global_mw, addr, body_limit)
                                    .await;
                            Ok::<_, Infallible>(response)
                        }
                    });

                    #[cfg(feature = "tls")]
                    if let Some(acceptor) = tls_acceptor {
                        let tls_stream = match tokio::time::timeout(
                            Duration::from_secs(10),
                            acceptor.accept(stream),
                        )
                        .await
                        {
                            Ok(Ok(s)) => s,
                            // TLS handshake failed or timed out — drop the connection silently.
                            Ok(Err(_)) | Err(_) => return,
                        };
                        drive_connection(TokioIo::new(tls_stream), service, rx).await;
                        return;
                    }

                    drive_connection(TokioIo::new(stream), service, rx).await;
                });
            }
            _ = &mut shutdown => {
                #[cfg(feature = "logging")]
                tracing::info!("shutdown signal received, draining connections");

                break;
            }
        }
    }

    // Signal all connections to stop accepting new requests
    let _ = shutdown_tx.send(true);

    // Drain with timeout
    let drain = async { while connections.join_next().await.is_some() {} };

    match tokio::time::timeout(shutdown_timeout, drain).await {
        Ok(()) => {
            #[cfg(feature = "logging")]
            tracing::info!("all connections drained, shutting down");
        }
        Err(_) => {
            #[cfg(feature = "logging")]
            {
                let remaining = connections.len();
                tracing::warn!(
                    "shutdown timeout reached, dropping {remaining} remaining connection(s)"
                );
            }

            connections.abort_all();
        }
    }

    // Run shutdown hooks concurrently with a timeout
    if !shutdown_hooks.is_empty() {
        let mut shutdown_set = JoinSet::new();
        for hook in shutdown_hooks {
            shutdown_set.spawn(hook());
        }

        match tokio::time::timeout(Duration::from_secs(10), async {
            while shutdown_set.join_next().await.is_some() {}
        })
        .await
        {
            Ok(()) => {
                #[cfg(feature = "logging")]
                tracing::info!("all shutdown hooks completed");
            }
            Err(_) => {
                #[cfg(feature = "logging")]
                {
                    let remaining = shutdown_set.len();
                    tracing::warn!("shutdown hook timeout, {remaining} hook(s) still running");
                }

                shutdown_set.abort_all();
            }
        }
    }
}

/// Drive a single connection to completion, serving HTTP/1.1 or HTTP/2
/// (auto-detected) over any transport that implements hyper's `Read` +
/// `Write` traits — a plain `TcpStream` or a TLS-wrapped stream both
/// qualify, which is what lets the TLS and non-TLS paths in [`serve`]
/// share this logic despite producing different concrete I/O types.
///
/// On `rx` signaling a graceful shutdown, in-flight requests on this
/// connection are allowed to finish before it closes.
async fn drive_connection<I, S>(io: I, service: S, mut rx: tokio::sync::watch::Receiver<bool>)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    S: hyper::service::Service<
            hyper::Request<hyper::body::Incoming>,
            Response = hyper::Response<Full<Bytes>>,
            Error = Infallible,
        > + Send
        + 'static,
    S::Future: Send,
{
    let builder = auto::Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(io, service);
    tokio::pin!(conn);

    tokio::select! {
        result = &mut conn => {
            if let Err(err) = result {
                let err_string = err.to_string();
                if !err_string.contains("incomplete") {
                    eprintln!("connection error: {err}");
                }
            }
        }
        _ = rx.changed() => {
            conn.as_mut().graceful_shutdown();
            if let Err(err) = conn.await {
                let err_string = err.to_string();
                if !err_string.contains("incomplete") {
                    eprintln!("connection error during drain: {err}");
                }
            }
        }
    }
}

/// Route a hyper request through the router, run the combined middleware
/// chain, and call the matched handler.
///
/// `body_limit` caps the request body size in bytes; bodies exceeding it
/// are rejected with `413 Payload Too Large` before routing occurs.
async fn handle_request(
    router: &Router,
    hyper_req: hyper::Request<hyper::body::Incoming>,
    state: Arc<TypeMap>,
    global_middleware: &[Arc<dyn Middleware>],
    peer_addr: std::net::SocketAddr,
    body_limit: usize,
) -> hyper::Response<Full<Bytes>> {
    let (parts, incoming) = hyper_req.into_parts();

    let limited = Limited::new(incoming, body_limit);
    let body = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            if e.downcast_ref::<LengthLimitError>().is_some() {
                return hyper::Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(r#"{"error":"Payload too large"}"#)))
                    .unwrap();
            }
            return hyper::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Full::new(Bytes::from("Failed to read request body")))
                .unwrap();
        }
    };

    let mut request = Request::new(
        parts.method,
        parts.uri,
        parts.headers,
        Vec::new(), // params are set inside handle_app_request
        body,
        state,
    );
    request.set_peer_ip(peer_addr.ip().to_string());

    handle_app_request(router, request, global_middleware)
        .await
        .into_hyper()
}

/// Route a request through the router and middleware chain, returning
/// a framework [`Response`](crate::response::Response).
///
/// This is the core routing logic shared by both the TCP server path
/// and the in-memory `TestClient` (added in a later phase). The TCP
/// path converts from hyper types before calling this function and
/// converts back after.
pub(crate) async fn handle_app_request(
    router: &Router,
    request: Request,
    global_middleware: &[Arc<dyn Middleware>],
) -> crate::response::Response {
    let method = request.method().clone();
    let path = request.path().to_string();

    if let Some(route_match) = router.find(&method, &path) {
        let mut request = request;
        request.set_params(route_match.params);

        let mut all_middleware: Vec<Arc<dyn Middleware>> = Vec::new();
        all_middleware.extend_from_slice(global_middleware);
        all_middleware.extend_from_slice(route_match.middleware);

        let ctx = crate::context::Context::new(request);
        let result =
            crate::middleware::run_middleware_chain(&all_middleware, route_match.handler, ctx)
                .await;

        return match result {
            Ok(response) => response,
            Err(err) => {
                use crate::response::IntoResponse;
                err.into_response()
            }
        };
    }

    let allowed = router.allowed_methods_for_path(&path);
    if !allowed.is_empty() {
        // The path is registered, just not for this method (e.g. a
        // preflight `OPTIONS` request for a path that only registers
        // `GET`). Global middleware *and* the route's own (e.g.
        // group-scoped) middleware still run, so middleware like
        // `Cors` can intercept it regardless of whether it was
        // registered globally or on a `.group(...)`; if nothing
        // short-circuits, the fallback handler below produces a 405
        // with an `Allow` header listing the methods that are handled.
        let route_middleware = router.find_middleware_for_path(&path).unwrap_or(&[]);

        let allow_value = allowed
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        use crate::handler::IntoHandler;
        let method_not_allowed_handler: Arc<dyn crate::handler::Handler> =
            (move |_req: Request| {
                use crate::response::IntoResponse;
                let mut resp =
                    crate::error::Error::method_not_allowed("Method Not Allowed").into_response();
                resp.set_header("allow", &allow_value);
                resp
            })
            .into_handler()
            .into();

        let mut all_middleware: Vec<Arc<dyn Middleware>> = Vec::new();
        all_middleware.extend_from_slice(global_middleware);
        all_middleware.extend_from_slice(route_middleware);

        let ctx = crate::context::Context::new(request);
        let result = crate::middleware::run_middleware_chain(
            &all_middleware,
            method_not_allowed_handler,
            ctx,
        )
        .await;

        match result {
            Ok(response) => response,
            Err(err) => {
                use crate::response::IntoResponse;
                err.into_response()
            }
        }
    } else {
        use crate::response::IntoResponse;
        crate::error::Error::not_found("Not Found").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::handler::IntoHandler;

    #[tokio::test]
    async fn handle_app_request_routes_correctly() {
        let mut router = Router::new();
        router.add(
            http::Method::GET,
            "/hello",
            (|_req: crate::request::Request| "world").into_handler(),
        );
        let state = Arc::new(TypeMap::new());
        let req = crate::request::Request::new(
            http::Method::GET,
            "/hello".parse().unwrap(),
            http::HeaderMap::new(),
            Vec::new(),
            bytes::Bytes::new(),
            state.clone(),
        );
        let resp = handle_app_request(&router, req, &[]).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.body_bytes(), b"world");
    }

    #[tokio::test]
    async fn handle_app_request_returns_404() {
        let router = Router::new();
        let state = Arc::new(TypeMap::new());
        let req = crate::request::Request::new(
            http::Method::GET,
            "/missing".parse().unwrap(),
            http::HeaderMap::new(),
            Vec::new(),
            bytes::Bytes::new(),
            state.clone(),
        );
        let resp = handle_app_request(&router, req, &[]).await;
        assert_eq!(resp.status(), http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_app_request_runs_middleware() {
        async fn tag(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Tag", "tested");
            Ok(resp)
        }
        let mut router = Router::new();
        router.add(
            http::Method::GET,
            "/",
            (|_req: crate::request::Request| "ok").into_handler(),
        );
        let mw: Vec<Arc<dyn crate::middleware::Middleware>> = vec![Arc::new(tag)];
        let state = Arc::new(TypeMap::new());
        let req = crate::request::Request::new(
            http::Method::GET,
            "/".parse().unwrap(),
            http::HeaderMap::new(),
            Vec::new(),
            bytes::Bytes::new(),
            state.clone(),
        );
        let resp = handle_app_request(&router, req, &mw).await;
        assert_eq!(resp.body_bytes(), b"ok");
        assert_eq!(
            resp.headers().get("X-Tag").unwrap().to_str().unwrap(),
            "tested"
        );
    }

    /// Helper: start an app on a random port, return the base URL and abort handle.
    async fn start_test_server(app: App) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let handle = tokio::spawn(async move {
            #[cfg(not(feature = "tls"))]
            let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit) =
                app.into_parts();
            #[cfg(feature = "tls")]
            let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, _tls) =
                app.into_parts();
            serve(
                router,
                listener,
                Arc::new(state),
                middleware,
                std::future::pending::<()>(),
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        // Give the server a moment to start accepting connections
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (base_url, handle)
    }

    #[tokio::test]
    async fn serve_stops_accepting_on_shutdown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let router = Router::new();
        let state = Arc::new(TypeMap::new());

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                state,
                vec![],
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_secs(5),
                vec![],
                2_097_152,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        // Send shutdown signal
        tx.send(()).unwrap();

        // serve() should return
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("serve did not return after shutdown signal")
            .expect("serve task panicked");
    }

    #[tokio::test]
    async fn in_flight_request_completes_during_shutdown() {
        let app = App::new().get("/slow", |_req: crate::request::Request| async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            "done"
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        #[cfg(not(feature = "tls"))]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, body_limit) =
            app.into_parts();
        #[cfg(feature = "tls")]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, body_limit, _tls) =
            app.into_parts();
        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                Arc::new(state),
                middleware,
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_secs(5),
                vec![],
                body_limit,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        // Wait for server to be ready
        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => break,
                Err(_) => tokio::task::yield_now().await,
            }
        }

        // Start a slow request on its own task so it actually begins
        // polling (a bare future does nothing until awaited or spawned).
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/slow");
        let resp_task = tokio::spawn(async move { client.get(&url).send().await });

        // Brief pause to let the request reach the handler
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Trigger shutdown while request is in flight
        tx.send(()).unwrap();

        // The slow request should still complete
        let resp = tokio::time::timeout(std::time::Duration::from_secs(3), resp_task)
            .await
            .expect("response timed out")
            .expect("request task panicked")
            .expect("request failed");
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "done");

        // Server should exit cleanly
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("serve did not return")
            .expect("serve panicked");
    }

    #[tokio::test]
    async fn shutdown_aborts_connections_past_timeout() {
        // Handler sleeps far longer than the shutdown_timeout below, so the
        // connection can never drain cleanly — this exercises the
        // `Err(_) => { connections.abort_all(); }` branch in `serve()`.
        let app = App::new().get("/slow", |_req: crate::request::Request| async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            "done"
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        #[cfg(not(feature = "tls"))]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, body_limit) =
            app.into_parts();
        #[cfg(feature = "tls")]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, body_limit, _tls) =
            app.into_parts();
        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                Arc::new(state),
                middleware,
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_millis(200),
                vec![],
                body_limit,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        // Wait for server to be ready
        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => break,
                Err(_) => tokio::task::yield_now().await,
            }
        }

        // Start a slow request on its own task so it's genuinely in flight
        // (a bare future does nothing until awaited or spawned). It will
        // never finish before the server gives up on it.
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/slow");
        let _resp_task = tokio::spawn(async move { client.get(&url).send().await });

        // Brief pause to let the request reach the handler
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Trigger shutdown — the handler needs ~4.95s more to finish, but
        // shutdown_timeout is only 200ms.
        tx.send(()).unwrap();

        // serve() must abort the stuck connection and return promptly —
        // well under the handler's 5s sleep — rather than hanging until it
        // finishes.
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("serve did not return within the abort window")
            .expect("serve panicked");
    }

    #[tokio::test]
    async fn serves_hello_world() {
        let app = App::new().get("/", |_req: crate::request::Request| "Hello World");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "Hello World");

        handle.abort();
    }

    #[tokio::test]
    async fn returns_404_for_unmatched_route() {
        // Unmatched routes now render through the same `Error::into_response`
        // path as every other error (see `handle_app_request`), so in dev
        // mode the body is the dev HTML error page rather than plain text.
        let app = App::new().get("/", |_req: crate::request::Request| "home");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/nonexistent")).await.unwrap();
        assert_eq!(resp.status(), 404);
        let body = resp.text().await.unwrap();
        assert!(body.contains("Not Found"), "expected 'Not Found' in {body}");

        handle.abort();
    }

    #[tokio::test]
    async fn returns_405_for_wrong_method() {
        let app = App::new().get("/users", |_req: crate::request::Request| "users");
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client.post(format!("{url}/users")).send().await.unwrap();
        assert_eq!(resp.status(), 405);
        assert_eq!(resp.headers().get("allow").unwrap(), "GET");

        handle.abort();
    }

    #[tokio::test]
    async fn returns_405_with_all_methods_in_allow_header() {
        let app = App::new()
            .get("/items", |_req: crate::request::Request| "list")
            .post("/items", |_req: crate::request::Request| "create")
            .delete("/items", |_req: crate::request::Request| "delete");
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client.put(format!("{url}/items")).send().await.unwrap();
        assert_eq!(resp.status(), 405);
        assert_eq!(resp.headers().get("allow").unwrap(), "DELETE, GET, POST");

        handle.abort();
    }

    #[tokio::test]
    async fn returns_404_for_genuinely_unknown_path() {
        let app = App::new().get("/known", |_req: crate::request::Request| "ok");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/unknown")).await.unwrap();
        assert_eq!(resp.status(), 404);
        assert!(resp.headers().get("allow").is_none());

        handle.abort();
    }

    #[tokio::test]
    async fn extracts_path_params() {
        let app = App::new().get("/users/:id", |req: crate::request::Request| {
            format!("User {}", req.param("id").unwrap())
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/users/42")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "User 42");

        handle.abort();
    }

    #[tokio::test]
    async fn serves_multiple_routes() {
        let app = App::new()
            .get("/", |_req: crate::request::Request| "home")
            .get("/about", |_req: crate::request::Request| "about")
            .post("/echo", |_req: crate::request::Request| "echoed");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "home");

        let resp = reqwest::get(format!("{url}/about")).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "about");

        let client = reqwest::Client::new();
        let resp = client.post(format!("{url}/echo")).send().await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "echoed");

        handle.abort();
    }

    #[tokio::test]
    async fn async_handler_works_over_http() {
        let app = App::new().get("/async", |_req: crate::request::Request| async {
            "async response"
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/async")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "async response");

        handle.abort();
    }

    #[tokio::test]
    async fn content_type_is_set() {
        let app = App::new().get("/", |_req: crate::request::Request| "hello");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/plain; charset=utf-8");

        handle.abort();
    }

    #[tokio::test]
    async fn handler_receives_request_body() {
        let app = App::new().post("/echo", |req: crate::request::Request| {
            String::from_utf8_lossy(req.body()).to_string()
        });
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{url}/echo"))
            .body("hello body")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "hello body");

        handle.abort();
    }

    #[tokio::test]
    async fn rejects_oversized_body_with_413() {
        let server = App::new()
            .body_limit(64) // 64 bytes max
            .post("/echo", |req: crate::request::Request| {
                String::from_utf8_lossy(req.body()).to_string()
            })
            .spawn()
            .await;

        let big_body = "x".repeat(128); // 128 bytes > 64 limit
        let resp = server
            .post("/echo")
            .header("content-type", "text/plain")
            .body(big_body.as_bytes())
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(resp.text().contains("Payload too large"));
    }

    #[tokio::test]
    async fn accepts_body_under_limit() {
        let server = App::new()
            .body_limit(256)
            .post("/echo", |req: crate::request::Request| {
                String::from_utf8_lossy(req.body()).to_string()
            })
            .spawn()
            .await;

        let resp = server
            .post("/echo")
            .header("content-type", "text/plain")
            .body(b"hello")
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text(), "hello");
    }

    #[tokio::test]
    async fn default_body_limit_is_2mib_over_http() {
        let server = App::new()
            .post("/echo", |req: crate::request::Request| {
                req.body().len().to_string()
            })
            .spawn()
            .await;

        // Exactly at the 2 MiB default — should pass.
        let body = "x".repeat(2_097_152);
        let resp = server
            .post("/echo")
            .header("content-type", "text/plain")
            .body(body.as_bytes())
            .send()
            .await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn query_extractor_over_http() {
        use crate::extract::Query;
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Params {
            name: String,
        }

        let app = App::new().get("/greet", |q: Query<Params>| format!("Hello, {}!", q.name));
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/greet?name=Alice"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "Hello, Alice!");

        handle.abort();
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn json_extractor_over_http() {
        use crate::extract::Json;
        use serde::{Deserialize, Serialize};

        #[derive(Deserialize)]
        struct Input {
            x: i32,
            y: i32,
        }

        #[derive(Serialize)]
        struct Output {
            sum: i32,
        }

        let app = App::new().post("/add", |body: Json<Input>| {
            Json(Output {
                sum: body.x + body.y,
            })
        });
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{url}/add"))
            .header("content-type", "application/json")
            .body(r#"{"x": 3, "y": 4}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["sum"], 7);

        handle.abort();
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn json_bad_content_type_returns_415() {
        use crate::extract::Json;
        use serde::Deserialize;

        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct Data {
            value: String,
        }

        let app = App::new().post("/data", |_body: Json<Data>| "ok");
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{url}/data"))
            .body(r#"{"value":"test"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 415);

        handle.abort();
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn json_invalid_body_returns_400() {
        use crate::extract::Json;
        use serde::Deserialize;

        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct Data {
            value: i32,
        }

        let app = App::new().post("/data", |_body: Json<Data>| "ok");
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{url}/data"))
            .header("content-type", "application/json")
            .body(r#"{"value":"not a number"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);

        handle.abort();
    }

    #[tokio::test]
    async fn html_response_over_http() {
        use crate::response::Html;

        let app = App::new().get("/page", |_req: crate::request::Request| {
            Html("<h1>Hello</h1>".to_string())
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/page")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");
        assert_eq!(resp.text().await.unwrap(), "<h1>Hello</h1>");

        handle.abort();
    }

    #[tokio::test]
    async fn error_not_found_over_http() {
        use crate::error::Error;

        let app = App::new().get("/fail", |_req: crate::request::Request| {
            std::result::Result::<&str, Error>::Err(Error::not_found("item not found"))
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/fail")).await.unwrap();
        assert_eq!(resp.status(), 404);

        handle.abort();
    }

    #[tokio::test]
    async fn error_auto_500_over_http() {
        use crate::error;

        let app = App::new().get("/crash", |_req: crate::request::Request| {
            fn might_fail() -> error::Result<String> {
                let bad_bytes = vec![0xFF_u8];
                let _ = std::str::from_utf8(&bad_bytes)?;
                Ok("ok".to_string())
            }
            might_fail()
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/crash")).await.unwrap();
        assert_eq!(resp.status(), 500);

        handle.abort();
    }

    #[tokio::test]
    async fn result_ok_over_http() {
        use crate::error;

        let app = App::new().get("/ok", |_req: crate::request::Request| {
            error::Result::Ok("all good")
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/ok")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "all good");

        handle.abort();
    }

    #[tokio::test]
    async fn state_extractor_over_http() {
        let app = App::new()
            .provide(42_u32)
            .get("/num", |n: crate::state::State<u32>| format!("num: {}", *n));

        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/num")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "num: 42");

        handle.abort();
    }

    #[tokio::test]
    async fn multiple_state_types_over_http() {
        let app = App::new()
            .provide(42_u32)
            .provide(String::from("hello"))
            .get(
                "/both",
                |n: crate::state::State<u32>, s: crate::state::State<String>| {
                    format!("{}: {}", *s, *n)
                },
            );

        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/both")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "hello: 42");

        handle.abort();
    }

    #[tokio::test]
    async fn missing_state_returns_500() {
        let app = App::new().get("/fail", |_n: crate::state::State<u32>| "unreachable");

        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/fail")).await.unwrap();
        assert_eq!(resp.status(), 500);

        handle.abort();
    }

    #[tokio::test]
    async fn state_with_custom_struct_over_http() {
        #[derive(Clone)]
        struct Config {
            greeting: String,
        }

        let app = App::new()
            .provide(Config {
                greeting: "Hey".into(),
            })
            .get("/greet", |cfg: crate::state::State<Config>| {
                format!("{}, world!", cfg.greeting)
            });

        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/greet")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "Hey, world!");

        handle.abort();
    }

    #[tokio::test]
    async fn global_middleware_runs() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static MW_RAN: AtomicBool = AtomicBool::new(false);

        async fn test_mw(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            MW_RAN.store(true, Ordering::SeqCst);
            next.run(ctx).await
        }

        MW_RAN.store(false, Ordering::SeqCst);
        let app = App::new()
            .use_mw(test_mw)
            .get("/", |_req: crate::request::Request| "hello");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "hello");
        assert!(MW_RAN.load(Ordering::SeqCst));

        handle.abort();
    }

    #[tokio::test]
    async fn middleware_modifies_response() {
        async fn add_header(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Custom", "middleware");
            Ok(resp)
        }

        let app = App::new()
            .use_mw(add_header)
            .get("/", |_req: crate::request::Request| "hello");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("X-Custom").unwrap().to_str().unwrap(),
            "middleware"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn middleware_short_circuits() {
        async fn blocker(
            _ctx: crate::context::Context,
            _next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            Err(crate::error::Error::unauthorized("no access"))
        }

        let app = App::new()
            .use_mw(blocker)
            .get("/", |_req: crate::request::Request| "unreachable");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 401);

        handle.abort();
    }

    #[tokio::test]
    async fn no_middleware_works() {
        let app = App::new().get("/", |_req: crate::request::Request| "no middleware");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "no middleware");

        handle.abort();
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn error_json_body_in_prod_mode() {
        use crate::error::Error;

        std::env::set_var("LADOO_ENV", "production");
        let app = App::new().get("/err", |_req: crate::request::Request| {
            std::result::Result::<&str, Error>::Err(Error::bad_request("invalid"))
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/err")).await.unwrap();
        assert_eq!(resp.status(), 400);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid");
        assert_eq!(body["status"], 400);
        std::env::remove_var("LADOO_ENV");

        handle.abort();
    }

    #[tokio::test]
    async fn group_with_middleware_over_http() {
        async fn add_header(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Group", "admin");
            Ok(resp)
        }

        let app = App::new()
            .get("/public", |_req: crate::request::Request| "public")
            .group("/admin", |r| {
                r.use_mw(add_header)
                    .get("/dashboard", |_req: crate::request::Request| "dashboard")
            });
        let (url, handle) = start_test_server(app).await;

        // Public route — no group middleware
        let public_resp = reqwest::get(format!("{url}/public")).await.unwrap();
        assert!(public_resp.headers().get("X-Group").is_none());
        assert_eq!(public_resp.text().await.unwrap(), "public");

        // Admin route — group middleware adds header
        let resp = reqwest::get(format!("{url}/admin/dashboard"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("X-Group").unwrap().to_str().unwrap(),
            "admin"
        );
        assert_eq!(resp.text().await.unwrap(), "dashboard");

        handle.abort();
    }

    #[tokio::test]
    async fn mounted_router_over_http() {
        let api = crate::router::Router::new()
            .get("/items", |_req: crate::request::Request| "items list");

        let app = App::new().mount("/api", api);
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/api/items")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "items list");

        handle.abort();
    }

    #[tokio::test]
    async fn multiple_global_middleware_execute_in_order() {
        async fn mw1(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW1", "yes");
            Ok(resp)
        }
        async fn mw2(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW2", "yes");
            Ok(resp)
        }

        let app = App::new()
            .use_mw(mw1)
            .use_mw(mw2)
            .get("/", |_req: crate::request::Request| "hello");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert!(resp.headers().contains_key("X-MW1"));
        assert!(resp.headers().contains_key("X-MW2"));

        handle.abort();
    }

    #[tokio::test]
    async fn middleware_does_not_run_on_404() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static MW_RAN: AtomicBool = AtomicBool::new(false);

        async fn tracker(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            MW_RAN.store(true, Ordering::SeqCst);
            next.run(ctx).await
        }

        MW_RAN.store(false, Ordering::SeqCst);
        let app = App::new()
            .use_mw(tracker)
            .get("/exists", |_req: crate::request::Request| "here");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/nonexistent")).await.unwrap();
        assert_eq!(resp.status(), 404);
        assert!(!MW_RAN.load(Ordering::SeqCst));

        handle.abort();
    }

    #[tokio::test]
    async fn nested_group_over_http() {
        let app = App::new().group("/api", |r| {
            r.get("/health", |_req: crate::request::Request| "ok")
                .get("/version", |_req: crate::request::Request| "1.0")
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/api/health")).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");

        let resp = reqwest::get(format!("{url}/api/version")).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "1.0");

        handle.abort();
    }

    #[tokio::test]
    async fn shutdown_hooks_run_after_drain() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let hook: crate::plugin::ShutdownHook = Box::new(move || {
            Box::pin(async move {
                flag_clone.store(true, Ordering::SeqCst);
            })
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let router = Router::new();
        let state = Arc::new(TypeMap::new());

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                state,
                vec![],
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_secs(5),
                vec![hook],
                2_097_152,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("serve did not return")
            .expect("serve panicked");

        assert!(flag.load(Ordering::SeqCst), "shutdown hook did not run");
    }

    #[tokio::test]
    async fn multiple_shutdown_hooks_all_run() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();
        let c3 = counter.clone();

        let hooks: Vec<crate::plugin::ShutdownHook> = vec![
            Box::new(move || {
                Box::pin(async move {
                    c1.fetch_add(1, Ordering::SeqCst);
                })
            }),
            Box::new(move || {
                Box::pin(async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                })
            }),
            Box::new(move || {
                Box::pin(async move {
                    c3.fetch_add(1, Ordering::SeqCst);
                })
            }),
        ];

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let router = Router::new();
        let state = Arc::new(TypeMap::new());

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                state,
                vec![],
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_secs(5),
                hooks,
                2_097_152,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("serve did not return")
            .expect("serve panicked");

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn shutdown_hooks_aborted_past_timeout() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let fast_flag = Arc::new(AtomicBool::new(false));
        let fast_clone = fast_flag.clone();

        let fast_hook: crate::plugin::ShutdownHook = Box::new(move || {
            Box::pin(async move {
                fast_clone.store(true, Ordering::SeqCst);
            })
        });

        let slow_hook: crate::plugin::ShutdownHook = Box::new(|| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            })
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let router = Router::new();
        let state = Arc::new(TypeMap::new());

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            serve(
                router,
                listener,
                state,
                vec![],
                async {
                    rx.await.ok();
                },
                std::time::Duration::from_secs(5),
                vec![fast_hook, slow_hook],
                2_097_152,
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        tx.send(()).unwrap();

        // serve() should still return within a reasonable time despite
        // the slow hook — the 10s hook timeout + abort kicks in.
        tokio::time::timeout(std::time::Duration::from_secs(15), handle)
            .await
            .expect("serve hung on slow shutdown hook")
            .expect("serve panicked");

        assert!(
            fast_flag.load(Ordering::SeqCst),
            "fast hook should have run"
        );
    }
}
