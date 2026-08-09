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
use std::sync::Arc;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::request::Request;
use crate::router::Router;

/// Start serving HTTP requests using the given router and listener.
///
/// This is the core server loop. It accepts connections, routes requests,
/// and sends responses. Each connection is handled in a separate Tokio task.
pub(crate) async fn serve(router: Router, listener: TcpListener) {
    let router = Arc::new(router);

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let router = router.clone();
        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            let router = router.clone();

            let service = service_fn(move |hyper_req: hyper::Request<hyper::body::Incoming>| {
                let router = router.clone();
                async move {
                    let response = handle_request(&router, hyper_req).await;
                    Ok::<_, Infallible>(response)
                }
            });

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                // Connection errors (client disconnect, etc.) are normal
                if !err.is_incomplete_message() {
                    eprintln!("connection error: {err}");
                }
            }
        });
    }
}

/// Route a hyper request through the router and call the matched handler.
async fn handle_request(
    router: &Router,
    hyper_req: hyper::Request<hyper::body::Incoming>,
) -> hyper::Response<Full<Bytes>> {
    let method = hyper_req.method().clone();
    let uri = hyper_req.uri().clone();
    let headers = hyper_req.headers().clone();
    let path = uri.path();

    match router.find(&method, path) {
        Some(route_match) => {
            let request = Request::new(method, uri, headers, route_match.params);
            let response = route_match.handler.call(request).await;
            response.into_hyper()
        }
        None => not_found_response(),
    }
}

fn not_found_response() -> hyper::Response<Full<Bytes>> {
    hyper::Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from("Not Found")))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    /// Helper: start an app on a random port, return the base URL and abort handle.
    async fn start_test_server(app: App) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let handle = tokio::spawn(async move {
            serve(app.into_router(), listener).await;
        });

        // Give the server a moment to start accepting connections
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (base_url, handle)
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
        let app = App::new().get("/", |_req: crate::request::Request| "home");
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/nonexistent")).await.unwrap();
        assert_eq!(resp.status(), 404);
        assert_eq!(resp.text().await.unwrap(), "Not Found");

        handle.abort();
    }

    #[tokio::test]
    async fn returns_404_for_wrong_method() {
        let app = App::new().get("/users", |_req: crate::request::Request| "users");
        let (url, handle) = start_test_server(app).await;

        let client = reqwest::Client::new();
        let resp = client.post(format!("{url}/users")).send().await.unwrap();
        assert_eq!(resp.status(), 404);

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
}
