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
    pub(crate) fn new(
        router: Router,
        state: TypeMap,
        global_middleware: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self {
            router: Arc::new(router),
            state: Arc::new(state),
            global_middleware: global_middleware.into(),
        }
    }

    /// Start building a GET request.
    pub fn get(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::GET, path)
    }

    /// Start building a POST request.
    pub fn post(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::POST, path)
    }

    /// Start building a PUT request.
    pub fn put(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::PUT, path)
    }

    /// Start building a DELETE request.
    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::DELETE, path)
    }

    /// Start building a PATCH request.
    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::PATCH, path)
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
}

impl<'a> TestRequest<'a> {
    fn new(client: &'a TestClient, method: Method, path: &str) -> Self {
        Self {
            client,
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
        let uri: http::Uri = self.path.parse().expect("invalid test path");
        let request = crate::request::Request::new(
            self.method,
            uri,
            self.headers,
            Vec::new(),
            self.body,
            self.client.state.clone(),
        );
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
    fn new(response: crate::response::Response) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
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
            .get("/search", |req: Request| {
                format!("path={}", req.uri())
            })
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
        let client = App::test()
            .get("/", |_req: Request| "raw")
            .into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.body_bytes(), b"raw");
    }
}
