//! The application builder.
//!
//! [`App`] is the entry point for building a Ladoo application. It provides
//! a fluent API for registering routes and (in later phases) middleware, state, and plugins.
//!
//! # Examples
//!
//! ```rust
//! use ladoo::app::App;
//! use ladoo::request::Request;
//!
//! # fn main() {
//! let _app = App::new()
//!     .get("/", |_: Request| "Hello World")
//!     .get("/users/:id", |req: Request| {
//!         let id = req.param("id").unwrap_or("0");
//!         format!("User {id}")
//!     });
//! # }
//! ```

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::handler::IntoHandler;
use crate::middleware::Middleware;
use crate::router::Router;
use crate::state::TypeMap;

/// The application builder.
///
/// `App` is the main entry point for building a Ladoo application.
/// Use the builder pattern to register routes, then start the server.
///
/// # Examples
///
/// ```rust
/// use ladoo::app::App;
/// use ladoo::request::Request;
///
/// # fn main() {
/// let _app = App::new()
///     .get("/", |_: Request| "Hello World");
/// # }
/// ```
pub struct App {
    router: Router,
    state: TypeMap,
    global_middleware: Vec<Arc<dyn Middleware>>,
}

impl App {
    /// Create a new application with no routes.
    pub fn new() -> Self {
        Self {
            router: Router::new(),
            state: TypeMap::new(),
            global_middleware: Vec::new(),
        }
    }

    /// Add a global middleware that runs on every matched route.
    ///
    /// Middleware are executed in the order they are registered (outer
    /// to inner) — the first middleware registered is the outermost
    /// layer, wrapping every middleware and handler registered after it.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// async fn logger(ctx: Context, next: Next) -> Result<Response> {
    ///     let resp = next.run(ctx).await?;
    ///     Ok(resp)
    /// }
    ///
    /// App::new().use_mw(logger).get("/", handler);
    /// ```
    pub fn use_mw<M: Middleware + 'static>(mut self, middleware: M) -> Self {
        self.global_middleware.push(Arc::new(middleware));
        self
    }

    /// Register a value for dependency injection.
    ///
    /// Any type that is `Send + Sync + 'static` can be provided. Extract
    /// it in handlers with [`State<T>`](crate::state::State). Providing a
    /// second value of the same type replaces the first.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::app::App;
    ///
    /// let _app = App::new()
    ///     .provide(42_u32)
    ///     .provide(String::from("hello"));
    /// ```
    pub fn provide<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.state.insert(value);
        self
    }

    /// Register a handler for GET requests to the given path.
    pub fn get<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.router
            .add(http::Method::GET, path, handler.into_handler());
        self
    }

    /// Register a handler for POST requests to the given path.
    pub fn post<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.router
            .add(http::Method::POST, path, handler.into_handler());
        self
    }

    /// Register a handler for PUT requests to the given path.
    pub fn put<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.router
            .add(http::Method::PUT, path, handler.into_handler());
        self
    }

    /// Register a handler for DELETE requests to the given path.
    pub fn delete<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.router
            .add(http::Method::DELETE, path, handler.into_handler());
        self
    }

    /// Register a handler for PATCH requests to the given path.
    pub fn patch<H, M>(mut self, path: &str, handler: H) -> Self
    where
        H: IntoHandler<M>,
    {
        self.router
            .add(http::Method::PATCH, path, handler.into_handler());
        self
    }

    /// Create a group of routes under a shared prefix.
    ///
    /// Routes added inside the closure are prefixed with the given path.
    /// Middleware added via `use_mw()` on the group's router applies only
    /// to routes in that group.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// App::new()
    ///     .group("/api/v1", |r| {
    ///         r.get("/users", list_users)
    ///          .post("/users", create_user)
    ///          .use_mw(auth)
    ///     })
    /// ```
    pub fn group<F>(mut self, prefix: &str, builder: F) -> Self
    where
        F: FnOnce(Router) -> Router,
    {
        let sub_router = builder(Router::new());
        self.router.merge_from(prefix, sub_router);
        self
    }

    /// Mount a standalone router under a prefix.
    ///
    /// All routes from the given router are added with the prefix
    /// prepended. Per-route middleware on the mounted router is preserved.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let api = Router::new()
    ///     .get("/items", list_items)
    ///     .post("/items", create_item);
    ///
    /// App::new().mount("/api", api);
    /// ```
    pub fn mount(mut self, prefix: &str, router: Router) -> Self {
        self.router.merge_from(prefix, router);
        self
    }

    /// Consume the App and return the inner router.
    ///
    /// Used internally by tests to access routes without also needing
    /// application state. Discards any state registered with
    /// [`App::provide`] — use [`App::into_parts`] when state matters.
    #[cfg(test)]
    pub(crate) fn into_router(self) -> Router {
        self.router
    }

    /// Consume the App and return the inner router, application state, and
    /// global middleware stack.
    ///
    /// Used internally by the server to access routes, dependency
    /// injection state, and middleware together.
    pub(crate) fn into_parts(self) -> (Router, TypeMap, Vec<Arc<dyn Middleware>>) {
        (self.router, self.state, self.global_middleware)
    }

    /// Start the HTTP server, blocking the current thread.
    ///
    /// Creates a Tokio runtime internally — no `#[tokio::main]` needed.
    /// This is the simplest way to start a Ladoo app.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use ladoo::prelude::*;
    ///
    /// fn main() {
    ///     App::new()
    ///         .get("/", |_: Request| "Hello World")
    ///         .run("0.0.0.0:3000");
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the Tokio runtime cannot be created or the address cannot be bound.
    pub fn run(self, addr: &str) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create Tokio runtime");

        let addr: std::net::SocketAddr = addr
            .parse()
            .expect("invalid address — expected format like 0.0.0.0:3000");

        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("failed to bind to {addr}: {e}"));

            println!("Ladoo listening on http://{addr}");
            let (router, state, middleware) = self.into_parts();
            crate::server::serve(router, listener, std::sync::Arc::new(state), middleware).await;
        });
    }

    /// Start the HTTP server using a pre-bound listener.
    ///
    /// Useful for tests (bind to port 0 for a random port) and advanced
    /// use cases where you manage the Tokio runtime yourself.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use ladoo::prelude::*;
    /// use tokio::net::TcpListener;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
    ///     App::new()
    ///         .get("/", |_: Request| "Hello World")
    ///         .serve_listener(listener)
    ///         .await;
    /// }
    /// ```
    pub async fn serve_listener(self, listener: TcpListener) {
        let (router, state, middleware) = self.into_parts();
        crate::server::serve(router, listener, std::sync::Arc::new(state), middleware).await;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FromRequest;
    use crate::request::Request;
    use http::Method;

    #[tokio::test]
    async fn get_registers_route() {
        let app = App::new().get("/hello", |_req: Request| "Hello");
        let router = app.into_router();

        let m = router.find(&Method::GET, "/hello");
        assert!(m.is_some());

        let req = Request::test(Method::GET, "/hello");
        let resp = m.unwrap().handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"Hello");
    }

    #[tokio::test]
    async fn post_registers_route() {
        let app = App::new().post("/users", |_req: Request| "created");
        let router = app.into_router();

        assert!(router.find(&Method::POST, "/users").is_some());
        assert!(router.find(&Method::GET, "/users").is_none());
    }

    #[tokio::test]
    async fn put_registers_route() {
        let app = App::new().put("/users/:id", |_req: Request| "updated");
        let router = app.into_router();
        assert!(router.find(&Method::PUT, "/users/1").is_some());
    }

    #[tokio::test]
    async fn delete_registers_route() {
        let app = App::new().delete("/users/:id", |_req: Request| "deleted");
        let router = app.into_router();
        assert!(router.find(&Method::DELETE, "/users/1").is_some());
    }

    #[tokio::test]
    async fn patch_registers_route() {
        let app = App::new().patch("/users/:id", |_req: Request| "patched");
        let router = app.into_router();
        assert!(router.find(&Method::PATCH, "/users/1").is_some());
    }

    #[tokio::test]
    async fn chaining_multiple_routes() {
        let app = App::new()
            .get("/", |_req: Request| "home")
            .get("/about", |_req: Request| "about")
            .post("/contact", |_req: Request| "contact");

        let router = app.into_router();
        assert!(router.find(&Method::GET, "/").is_some());
        assert!(router.find(&Method::GET, "/about").is_some());
        assert!(router.find(&Method::POST, "/contact").is_some());
        assert!(router.find(&Method::GET, "/missing").is_none());
    }

    #[tokio::test]
    async fn async_handler_via_app() {
        let app = App::new().get("/async", |_req: Request| async { "async works" });
        let router = app.into_router();

        let m = router.find(&Method::GET, "/async").unwrap();
        let req = Request::test(Method::GET, "/async");
        let resp = m.handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"async works");
    }

    #[tokio::test]
    async fn handler_with_path_params() {
        let app = App::new().get("/users/:id", |req: Request| {
            format!("User {}", req.param("id").unwrap())
        });
        let router = app.into_router();

        let m = router.find(&Method::GET, "/users/42").unwrap();
        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(m.params.clone());
        let resp = m.handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"User 42");
    }

    struct PathStr(String);
    impl FromRequest for PathStr {
        fn from_request(
            req: &mut crate::request::Request,
        ) -> Result<Self, crate::response::Response> {
            Ok(PathStr(req.path().to_string()))
        }
    }

    #[test]
    fn app_accepts_extractor_handler() {
        let app = App::new().get("/", |path: PathStr| format!("got: {}", path.0));
        let router = app.into_router();
        assert!(router.find(&Method::GET, "/").is_some());
    }

    #[tokio::test]
    async fn app_extractor_handler_works() {
        let app = App::new().get("/test", |path: PathStr| format!("path: {}", path.0));
        let router = app.into_router();
        let m = router.find(&Method::GET, "/test").unwrap();
        let req = crate::request::Request::test(Method::GET, "/test");
        let resp = m.handler.call(req).await;
        assert_eq!(resp.body_bytes(), b"path: /test");
    }

    #[test]
    fn provide_stores_state() {
        let app = App::new().provide(42_u32);
        // We can't directly access state, but we can verify it builds
        let _ = app.into_router();
    }

    #[test]
    fn provide_multiple_types() {
        let app = App::new()
            .provide(42_u32)
            .provide(String::from("hello"))
            .provide(3.14_f64);
        let _ = app.into_router();
    }

    #[test]
    fn provide_chains_with_routes() {
        let app = App::new()
            .provide(42_u32)
            .get("/", |_req: Request| "hello")
            .provide(String::from("world"));
        let router = app.into_router();
        assert!(router.find(&Method::GET, "/").is_some());
    }

    #[test]
    fn into_parts_returns_router_and_state() {
        let app = App::new().provide(42_u32).get("/", |_req: Request| "hi");
        let (router, state, _middleware) = app.into_parts();
        assert!(router.find(&Method::GET, "/").is_some());
        assert_eq!(state.get::<u32>(), Some(&42));
    }

    #[test]
    fn use_mw_chains() {
        async fn noop(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
            Ok(next.run(ctx).await?)
        }
        let app = App::new()
            .use_mw(noop)
            .get("/", |_req: Request| "hello");
        let _ = app.into_parts();
    }

    #[test]
    fn group_adds_prefixed_routes() {
        let app = App::new().group("/api", |r| {
            r.get("/users", |_req: Request| "users")
                .post("/users", |_req: Request| "created")
        });
        let (router, _, _) = app.into_parts();
        assert!(router.find(&Method::GET, "/api/users").is_some());
        assert!(router.find(&Method::POST, "/api/users").is_some());
    }

    #[test]
    fn mount_adds_prefixed_routes() {
        let api = Router::new().get("/items", |_req: Request| "items");
        let app = App::new().mount("/api", api);
        let (router, _, _) = app.into_parts();
        assert!(router.find(&Method::GET, "/api/items").is_some());
    }
}
