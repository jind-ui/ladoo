//! Testing utilities for Ladoo applications.
//!
//! The [`TestClient`] lets you send requests through your app's middleware
//! and handlers **in-memory** — no TCP server, no ports, no waiting. Build
//! one from an [`App`](crate::app::App) with [`App::into_client`](crate::app::App::into_client):
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! #[tokio::test]
//! async fn hello() {
//!     let client = App::test()
//!         .get("/", |_: Request| "hello")
//!         .into_client();
//!
//!     let resp = client.get("/").send().await;
//!     assert_eq!(resp.status(), 200);
//!     assert_eq!(resp.text(), "hello");
//! }
//! ```

use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};

use crate::middleware::Middleware;
#[cfg(any(test, feature = "test-server"))]
use crate::plugin::ShutdownHook;
use crate::router::Router;
use crate::state::TypeMap;

/// An in-memory HTTP client for testing Ladoo apps.
///
/// Routes requests through the full middleware chain and handler
/// without opening a TCP connection. Create one with
/// [`App::into_client`](crate::app::App::into_client).
pub struct TestClient {
    router: Arc<Router>,
    state: Arc<TypeMap>,
    global_middleware: Arc<[Arc<dyn Middleware>]>,
}

impl TestClient {
    /// Create a test client from app parts.
    ///
    /// `state` must already be finalized via
    /// [`build_and_initialize_state`](crate::app::build_and_initialize_state)
    /// so that any `JobRunner` in state has been wired up before tests run.
    pub(crate) fn new(
        router: Router,
        state: Arc<TypeMap>,
        global_middleware: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self {
            router: Arc::new(router),
            state,
            global_middleware: global_middleware.into(),
        }
    }

    /// Start building a request with an arbitrary HTTP method.
    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, method, path)
    }

    /// Start building a GET request.
    pub fn get(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::GET, path)
    }

    /// Start building a POST request.
    pub fn post(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::POST, path)
    }

    /// Start building a PUT request.
    pub fn put(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PUT, path)
    }

    /// Start building a DELETE request.
    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::DELETE, path)
    }

    /// Start building a PATCH request.
    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PATCH, path)
    }
}

/// A request builder for the test client.
///
/// Build up a request with headers, body, and query parameters, then
/// call [`send`](TestRequest::send) to execute it in-memory.
pub struct TestRequest<'a> {
    client: &'a TestClient,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
    peer_ip: Option<String>,
}

impl<'a> TestRequest<'a> {
    fn new(client: &'a TestClient, method: Method, path: &str) -> Self {
        Self {
            client,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            peer_ip: None,
        }
    }

    /// Set a simulated client IP address for the request.
    ///
    /// Used to test IP-based rate limiting and other IP-dependent middleware.
    pub fn peer_ip(mut self, ip: &str) -> Self {
        self.peer_ip = Some(ip.to_string());
        self
    }

    /// Add a header to the request.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).expect("invalid header name"),
            http::header::HeaderValue::from_str(value).expect("invalid header value"),
        );
        self
    }

    /// Set the request body as raw bytes.
    pub fn body(mut self, body: &[u8]) -> Self {
        self.body = Bytes::copy_from_slice(body);
        self
    }

    /// Serialize `value` as JSON and set it as the request body.
    ///
    /// Also sets the `Content-Type` header to `application/json`.
    #[cfg(feature = "json")]
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Self {
        self.body = Bytes::from(serde_json::to_vec(value).expect("failed to serialize JSON"));
        self.headers.insert(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("application/json"),
        );
        self
    }

    /// Serialize `params` as a URL query string and append it to the path.
    #[cfg(feature = "json")]
    pub fn query<T: serde::Serialize>(mut self, params: &T) -> Self {
        let qs = serde_urlencoded::to_string(params).expect("failed to serialize query");
        if self.path.contains('?') {
            self.path = format!("{}&{qs}", self.path);
        } else {
            self.path = format!("{}?{qs}", self.path);
        }
        self
    }

    /// Send the request and return the response.
    ///
    /// The request is routed through the app's middleware chain and
    /// handler in-memory — no TCP connection is opened.
    pub async fn send(self) -> TestResponse {
        let uri: http::Uri = self
            .path
            .parse()
            .unwrap_or_else(|e| panic!("invalid test path {:?}: {e}", self.path));
        let mut request = crate::request::Request::new(
            self.method,
            uri,
            self.headers,
            Vec::new(),
            self.body,
            self.client.state.clone(),
        );
        if let Some(ip) = &self.peer_ip {
            request.set_peer_ip(ip.clone());
        }
        let response = crate::server::handle_app_request(
            &self.client.router,
            request,
            &self.client.global_middleware,
        )
        .await;
        TestResponse::new(response)
    }
}

/// The response from a test request.
///
/// Provides methods to inspect the status, headers, and body of the
/// response returned by the app's handler.
pub struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    pub(crate) fn new(response: crate::response::Response) -> Self {
        Self {
            status: response.status(),
            headers: response.headers().clone(),
            body: Bytes::copy_from_slice(response.body_bytes()),
        }
    }

    /// Returns the HTTP status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns a header value by name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Returns all response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns the `Content-Type` header value, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// Returns the response body as a UTF-8 string.
    ///
    /// # Panics
    ///
    /// Panics if the body is not valid UTF-8.
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).expect("response body is not valid UTF-8")
    }

    /// Deserialize the response body from JSON.
    ///
    /// # Panics
    ///
    /// Panics if the body cannot be deserialized as `T`.
    #[cfg(feature = "json")]
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("failed to deserialize JSON response")
    }

    /// Returns the response body as raw bytes.
    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }
}

#[cfg(any(test, feature = "test-server"))]
/// A test server running on a real TCP port.
///
/// Created by [`App::spawn`](crate::app::App::spawn). Starts a real
/// Hyper server on a random port. Requests go over TCP through the
/// full network stack — useful for integration tests that need to
/// exercise real HTTP behavior.
///
/// The server is stopped automatically when the `TestServer` is dropped.
pub struct TestServer {
    base_url: String,
    client: reqwest::Client,
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(any(test, feature = "test-server"))]
impl TestServer {
    /// Create and start a test server from app parts.
    ///
    /// `state` must already be finalized via
    /// [`build_and_initialize_state`](crate::app::build_and_initialize_state)
    /// so that any `JobRunner` in state has been wired up before the
    /// server starts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start(
        router: Router,
        state: Arc<TypeMap>,
        global_middleware: Vec<Arc<dyn Middleware>>,
        shutdown_timeout: std::time::Duration,
        shutdown_hooks: Vec<ShutdownHook>,
        body_limit: usize,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind test server");
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let handle = tokio::spawn(async move {
            crate::server::serve(
                router,
                listener,
                state,
                global_middleware,
                std::future::pending::<()>(),
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
                // `TestServer` never serves TLS — see the "No TLS on
                // TestServer" note on `App::spawn`. TLS integration
                // tests set up their own listener and call `serve`
                // directly instead.
                #[cfg(feature = "tls")]
                None,
            )
            .await;
        });

        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => break,
                Err(_) => tokio::task::yield_now().await,
            }
        }

        Self {
            base_url,
            client: reqwest::Client::new(),
            handle,
        }
    }

    /// Build the full URL for `path` against this server's base address.
    ///
    /// Useful when a test needs a raw URL string — e.g. to hand to a
    /// custom `reqwest::Client` — rather than going through the
    /// [`ServerTestRequest`] builder.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Start building a request with an arbitrary HTTP method.
    pub fn request(&self, method: Method, path: &str) -> ServerTestRequest<'_> {
        ServerTestRequest::new(self, method, path)
    }

    /// Start building a GET request.
    pub fn get(&self, path: &str) -> ServerTestRequest<'_> {
        self.request(Method::GET, path)
    }

    /// Start building a POST request.
    pub fn post(&self, path: &str) -> ServerTestRequest<'_> {
        self.request(Method::POST, path)
    }

    /// Start building a PUT request.
    pub fn put(&self, path: &str) -> ServerTestRequest<'_> {
        self.request(Method::PUT, path)
    }

    /// Start building a DELETE request.
    pub fn delete(&self, path: &str) -> ServerTestRequest<'_> {
        self.request(Method::DELETE, path)
    }

    /// Start building a PATCH request.
    pub fn patch(&self, path: &str) -> ServerTestRequest<'_> {
        self.request(Method::PATCH, path)
    }
}

#[cfg(any(test, feature = "test-server"))]
impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(any(test, feature = "test-server"))]
/// A request builder for the test server (real TCP).
///
/// Build up a request with headers, body, and query parameters, then
/// call [`send`](ServerTestRequest::send) to execute it over the network.
pub struct ServerTestRequest<'a> {
    server: &'a TestServer,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

#[cfg(any(test, feature = "test-server"))]
impl<'a> ServerTestRequest<'a> {
    fn new(server: &'a TestServer, method: Method, path: &str) -> Self {
        Self {
            server,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
    }

    /// Add a header to the request.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).expect("invalid header name"),
            http::header::HeaderValue::from_str(value).expect("invalid header value"),
        );
        self
    }

    /// Set the request body as raw bytes.
    pub fn body(mut self, body: &[u8]) -> Self {
        self.body = Bytes::copy_from_slice(body);
        self
    }

    /// Serialize `value` as JSON and set it as the request body.
    #[cfg(feature = "json")]
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Self {
        self.body = Bytes::from(serde_json::to_vec(value).expect("failed to serialize JSON"));
        self.headers.insert(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("application/json"),
        );
        self
    }

    /// Serialize `params` as a URL query string and append it to the path.
    #[cfg(feature = "json")]
    pub fn query<T: serde::Serialize>(mut self, params: &T) -> Self {
        let qs = serde_urlencoded::to_string(params).expect("failed to serialize query");
        if self.path.contains('?') {
            self.path = format!("{}&{qs}", self.path);
        } else {
            self.path = format!("{}?{qs}", self.path);
        }
        self
    }

    /// Send the request over TCP and return the response.
    pub async fn send(self) -> TestResponse {
        let url = format!("{}{}", self.server.base_url, self.path);
        let mut req_builder = self.server.client.request(self.method, &url);
        for (name, value) in &self.headers {
            req_builder = req_builder.header(name, value);
        }
        if !self.body.is_empty() {
            req_builder = req_builder.body(self.body.to_vec());
        }
        let resp = req_builder.send().await.expect("test request failed");
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.expect("failed to read response body");
        TestResponse {
            status,
            headers,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::request::Request;
    use http::StatusCode;

    #[tokio::test]
    async fn get_returns_response() {
        let client = App::test()
            .get("/hello", |_req: Request| "world")
            .into_client();
        let resp = client.get("/hello").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text(), "world");
    }

    #[tokio::test]
    async fn post_with_body() {
        let client = App::test()
            .post("/echo", |req: Request| {
                String::from_utf8_lossy(req.body()).to_string()
            })
            .into_client();
        let resp = client.post("/echo").body(b"hello").send().await;
        assert_eq!(resp.text(), "hello");
    }

    #[tokio::test]
    async fn not_found_returns_404() {
        let client = App::test()
            .get("/exists", |_req: Request| "here")
            .into_client();
        let resp = client.get("/missing").send().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn custom_header_in_request() {
        let client = App::test()
            .get("/check", |req: Request| {
                req.headers()
                    .get("X-Custom")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_else(|| "missing".into())
            })
            .into_client();
        let resp = client
            .get("/check")
            .header("X-Custom", "present")
            .send()
            .await;
        assert_eq!(resp.text(), "present");
    }

    #[tokio::test]
    async fn response_header_accessible() {
        async fn add_header(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Test", "value");
            Ok(resp)
        }
        let client = App::test()
            .use_mw(add_header)
            .get("/", |_req: Request| "ok")
            .into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.header("X-Test"), Some("value"));
    }

    #[tokio::test]
    async fn put_method() {
        let client = App::test()
            .put("/item", |_req: Request| "updated")
            .into_client();
        let resp = client.put("/item").send().await;
        assert_eq!(resp.text(), "updated");
    }

    #[tokio::test]
    async fn delete_method() {
        let client = App::test()
            .delete("/item/:id", |req: Request| {
                format!("deleted {}", req.param("id").unwrap())
            })
            .into_client();
        let resp = client.delete("/item/42").send().await;
        assert_eq!(resp.text(), "deleted 42");
    }

    #[tokio::test]
    async fn patch_method() {
        let client = App::test()
            .patch("/item", |_req: Request| "patched")
            .into_client();
        let resp = client.patch("/item").send().await;
        assert_eq!(resp.text(), "patched");
    }

    #[tokio::test]
    async fn middleware_runs_in_test_client() {
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
        let client = App::test()
            .use_mw(tracker)
            .get("/", |_req: Request| "ok")
            .into_client();
        let _resp = client.get("/").send().await;
        assert!(MW_RAN.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn state_accessible_in_test_client() {
        let client = App::test()
            .provide(42_u32)
            .get("/num", |n: crate::state::State<u32>| format!("{}", *n))
            .into_client();
        let resp = client.get("/num").send().await;
        assert_eq!(resp.text(), "42");
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn json_request_and_response() {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, Debug, Clone)]
        struct Item {
            name: String,
        }

        let client = App::test()
            .post("/item", |body: crate::extract::Json<Item>| {
                crate::extract::Json(body.0)
            })
            .into_client();
        let resp = client
            .post("/item")
            .json(&Item {
                name: "test".into(),
            })
            .send()
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let item: Item = resp.json();
        assert_eq!(item.name, "test");
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn query_string_request() {
        use serde::Serialize;

        #[derive(Serialize)]
        struct Params {
            q: String,
            page: u32,
        }

        let client = App::test()
            .get("/search", |req: Request| format!("path={}", req.uri()))
            .into_client();
        let resp = client
            .get("/search")
            .query(&Params {
                q: "rust".into(),
                page: 2,
            })
            .send()
            .await;
        let text = resp.text();
        assert!(text.contains("q=rust"), "expected q=rust in {text}");
        assert!(text.contains("page=2"), "expected page=2 in {text}");
    }

    #[tokio::test]
    async fn body_bytes_returns_raw() {
        let client = App::test().get("/", |_req: Request| "raw").into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.body_bytes(), b"raw");
    }

    #[tokio::test]
    async fn test_request_peer_ip_available_in_handler() {
        let client = App::test()
            .get("/ip", |req: Request| {
                req.peer_ip().unwrap_or("unknown").to_string()
            })
            .into_client();
        let resp = client.get("/ip").peer_ip("1.2.3.4").send().await;
        assert_eq!(resp.text(), "1.2.3.4");
    }

    #[tokio::test]
    async fn test_request_without_peer_ip() {
        let client = App::test()
            .get("/ip", |req: Request| {
                req.peer_ip().unwrap_or("unknown").to_string()
            })
            .into_client();
        let resp = client.get("/ip").send().await;
        assert_eq!(resp.text(), "unknown");
    }

    // --- TestServer (real TCP) tests ---

    #[tokio::test]
    async fn spawn_serves_over_tcp() {
        let server = App::test()
            .get("/hello", |_req: Request| "world")
            .spawn()
            .await;
        let resp = server.get("/hello").send().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text(), "world");
    }

    #[tokio::test]
    async fn spawn_post_with_body() {
        let server = App::test()
            .post("/echo", |req: Request| {
                String::from_utf8_lossy(req.body()).to_string()
            })
            .spawn()
            .await;
        let resp = server.post("/echo").body(b"hello tcp").send().await;
        assert_eq!(resp.text(), "hello tcp");
    }

    #[tokio::test]
    async fn spawn_404() {
        let server = App::test().get("/", |_req: Request| "home").spawn().await;
        let resp = server.get("/missing").send().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn spawn_with_middleware() {
        async fn tag(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-Spawned", "yes");
            Ok(resp)
        }
        let server = App::test()
            .use_mw(tag)
            .get("/", |_req: Request| "ok")
            .spawn()
            .await;
        let resp = server.get("/").send().await;
        assert_eq!(resp.header("X-Spawned"), Some("yes"));
    }

    #[tokio::test]
    async fn spawn_with_state() {
        let server = App::test()
            .provide(99_u32)
            .get("/num", |n: crate::state::State<u32>| format!("{}", *n))
            .spawn()
            .await;
        let resp = server.get("/num").send().await;
        assert_eq!(resp.text(), "99");
    }

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn spawn_json_roundtrip() {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, Debug, Clone)]
        struct Item {
            name: String,
        }

        let server = App::test()
            .post("/item", |body: crate::extract::Json<Item>| {
                crate::extract::Json(body.0)
            })
            .spawn()
            .await;
        let resp = server
            .post("/item")
            .json(&Item {
                name: "tcp-test".into(),
            })
            .send()
            .await;
        let item: Item = resp.json();
        assert_eq!(item.name, "tcp-test");
    }
}
