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
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::middleware::Middleware;
use crate::request::Request;
use crate::router::Router;
use crate::state::TypeMap;

/// Start serving HTTP requests using the given router, listener,
/// application state, and global middleware stack.
///
/// This is the core server loop. It accepts connections, routes requests,
/// and sends responses. Each connection is handled in a separate Tokio task.
/// `state` is shared across every request and is what powers the
/// [`State<T>`](crate::state::State) extractor. `global_middleware` runs on
/// every matched route, ahead of any route-specific middleware.
pub(crate) async fn serve(
    router: Router,
    listener: TcpListener,
    state: Arc<TypeMap>,
    global_middleware: Vec<Arc<dyn Middleware>>,
) {
    let router = Arc::new(router);
    let global_middleware: Arc<[Arc<dyn Middleware>]> = global_middleware.into();

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let router = router.clone();
        let state = state.clone();
        let global_mw = global_middleware.clone();
        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            let router = router.clone();
            let state = state.clone();
            let global_mw = global_mw.clone();

            let service = service_fn(move |hyper_req: hyper::Request<hyper::body::Incoming>| {
                let router = router.clone();
                let state = state.clone();
                let global_mw = global_mw.clone();
                async move {
                    let response = handle_request(&router, hyper_req, state, &global_mw).await;
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

/// Route a hyper request through the router, run the combined middleware
/// chain, and call the matched handler.
async fn handle_request(
    router: &Router,
    hyper_req: hyper::Request<hyper::body::Incoming>,
    state: Arc<TypeMap>,
    global_middleware: &[Arc<dyn Middleware>],
) -> hyper::Response<Full<Bytes>> {
    let (parts, incoming) = hyper_req.into_parts();
    let path = parts.uri.path();

    match router.find(&parts.method, path) {
        Some(route_match) => {
            let body = match incoming.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => {
                    return hyper::Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(Full::new(Bytes::from("Failed to read request body")))
                        .unwrap();
                }
            };
            let request = Request::new(
                parts.method,
                parts.uri,
                parts.headers,
                route_match.params,
                body,
                state,
            );

            let mut all_middleware: Vec<Arc<dyn Middleware>> = Vec::new();
            all_middleware.extend_from_slice(global_middleware);
            all_middleware.extend_from_slice(route_match.middleware);

            let ctx = crate::context::Context::new(request);
            let result = crate::middleware::run_middleware_chain(
                &all_middleware,
                route_match.handler,
                ctx,
            )
            .await;

            match result {
                Ok(response) => response.into_hyper(),
                Err(err) => {
                    use crate::response::IntoResponse;
                    err.into_response().into_hyper()
                }
            }
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
            let (router, state, middleware) = app.into_parts();
            serve(router, listener, Arc::new(state), middleware).await;
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

    #[cfg(feature = "json")]
    #[tokio::test]
    async fn query_extractor_over_http() {
        use crate::extract::Query;
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Params {
            name: String,
        }

        let app = App::new().get("/greet", |q: Query<Params>| {
            format!("Hello, {}!", q.name)
        });
        let (url, handle) = start_test_server(app).await;

        let resp = reqwest::get(format!("{url}/greet?name=Alice")).await.unwrap();
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
            Json(Output { sum: body.x + body.y })
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
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
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
        let app = App::new().provide(42_u32).provide(String::from("hello")).get(
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

        async fn test_mw(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
            MW_RAN.store(true, Ordering::SeqCst);
            Ok(next.run(ctx).await?)
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
        async fn add_header(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
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
        async fn blocker(_ctx: crate::context::Context, _next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
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
        let app = App::new()
            .get("/", |_req: crate::request::Request| "no middleware");
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
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert_eq!(ct, "application/json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid");
        assert_eq!(body["status"], 400);
        std::env::remove_var("LADOO_ENV");

        handle.abort();
    }

    #[tokio::test]
    async fn group_with_middleware_over_http() {
        async fn add_header(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
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
        let resp = reqwest::get(format!("{url}/admin/dashboard")).await.unwrap();
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
        async fn mw1(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
            let mut resp = next.run(ctx).await?;
            resp.set_header("X-MW1", "yes");
            Ok(resp)
        }
        async fn mw2(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
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

        async fn tracker(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
            MW_RAN.store(true, Ordering::SeqCst);
            Ok(next.run(ctx).await?)
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
        let app = App::new()
            .group("/api", |r| {
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
}
