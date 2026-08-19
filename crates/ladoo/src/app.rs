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
use crate::plugin::ShutdownHook;
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
    #[cfg(feature = "logging")]
    logging_config: crate::logging::LoggingConfig,
    shutdown_timeout: std::time::Duration,
    body_limit: usize,
    shutdown_hooks: Vec<ShutdownHook>,
    plugin_names: Vec<String>,
    health_registry: crate::health::HealthRegistry,
    health_config: crate::health::HealthConfig,
    #[cfg(feature = "tls")]
    tls_config: Option<crate::tls::TlsConfig>,
}

impl App {
    /// Create a new application with no routes.
    pub fn new() -> Self {
        Self {
            router: Router::new(),
            state: TypeMap::new(),
            global_middleware: Vec::new(),
            #[cfg(feature = "logging")]
            logging_config: Default::default(),
            shutdown_timeout: std::time::Duration::from_secs(30),
            body_limit: 2_097_152, // 2 MiB default
            shutdown_hooks: Vec::new(),
            plugin_names: Vec::new(),
            health_registry: crate::health::HealthRegistry::new(),
            health_config: crate::health::HealthConfig::new(),
            #[cfg(feature = "tls")]
            tls_config: None,
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
        self.state.insert_shared(value);
        self
    }

    /// Load configuration and provide it as application state.
    ///
    /// Calls [`Config::load()`](crate::config::Config::load) to read from environment variables,
    /// TOML files, and field defaults, then registers the result as
    /// [`State<T>`](crate::state::State). Panics at startup if
    /// configuration loading fails — a misconfigured app should never
    /// accept traffic.
    ///
    /// Users who prefer manual configuration can skip this method and
    /// call [`.provide()`](Self::provide) directly.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// #[derive(Config)]
    /// struct AppConfig {
    ///     #[config(default = 3000)]
    ///     port: u16,
    /// }
    ///
    /// App::new()
    ///     .config::<AppConfig>()
    ///     .get("/", |cfg: State<AppConfig>| {
    ///         format!("port: {}", cfg.port)
    ///     })
    ///     .run("0.0.0.0:3000");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if [`Config::load()`](crate::config::Config::load) returns an error.
    #[cfg(feature = "config")]
    pub fn config<T: crate::config::Config>(self) -> Self {
        let config = T::load().unwrap_or_else(|e| {
            panic!("configuration error: {e}");
        });
        self.provide(config)
    }

    /// Set pagination defaults and limits.
    ///
    /// The provided [`PaginationConfig`](crate::pagination::PaginationConfig) is
    /// stored as application state. [`Paginate`](crate::pagination::Paginate) and
    /// [`CursorParams`](crate::pagination::CursorParams) extractors read it
    /// automatically to apply default and maximum page sizes.
    ///
    /// If not called, hardcoded defaults apply (20 per page, 100 max).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::app::App;
    /// use ladoo::pagination::PaginationConfig;
    ///
    /// let _app = App::new()
    ///     .pagination(PaginationConfig::new()
    ///         .default_per_page(25)
    ///         .max_per_page(50)
    ///     );
    /// ```
    #[cfg(feature = "json")]
    pub fn pagination(self, config: crate::pagination::PaginationConfig) -> Self {
        self.provide(config)
    }

    /// Provide state that also registers a health check.
    ///
    /// The value is stored in application state (available via
    /// [`State<T>`](crate::state::State)) AND registered with the health
    /// endpoint. A clone of the value is used for health checking.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// #[async_trait]
    /// impl HealthCheckable for Database {
    ///     fn name(&self) -> &str { "postgres" }
    ///     async fn check(&self) -> Result<()> { self.ping().await }
    /// }
    ///
    /// App::new()
    ///     .provide_healthy(Database::connect(url).await?)
    ///     .get("/users", list_users);
    /// ```
    pub fn provide_healthy<T>(mut self, value: T) -> Self
    where
        T: crate::health::HealthCheckable + Clone + Send + Sync + 'static,
    {
        let health_clone = value.clone();
        self.state.insert_shared(value);
        self.health_registry
            .checks
            .push(std::sync::Arc::new(health_clone));
        self
    }

    /// Register a manual health check closure.
    ///
    /// Use for dependencies that don't implement [`HealthCheckable`](crate::health::HealthCheckable) —
    /// third-party types, external APIs, or ad-hoc checks.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// App::new()
    ///     .health("external-api", || async {
    ///         reqwest::get("https://api.example.com/ping").await?;
    ///         Ok(())
    ///     })
    /// ```
    pub fn health<F, Fut>(mut self, name: &str, check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = crate::error::Result<()>> + Send + 'static,
    {
        self.health_registry
            .closures
            .push(crate::health::HealthClosure {
                name: name.to_string(),
                check: std::sync::Arc::new(move || Box::pin(check())),
            });
        self
    }

    /// Configure the health endpoint.
    ///
    /// Controls the path, response format, metadata, and per-check timeout.
    /// If not called, defaults apply: path `/health`, detailed responses,
    /// no latency, 5-second check timeout.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::app::App;
    /// use ladoo::health::HealthConfig;
    ///
    /// let _app = App::new()
    ///     .health_config(HealthConfig::new()
    ///         .path("/healthz")
    ///         .detailed(false)
    ///     );
    /// ```
    pub fn health_config(mut self, config: crate::health::HealthConfig) -> Self {
        self.health_config = config;
        self
    }

    /// Set the default log level (e.g., `"debug"`, `"info"`, `"warn"`).
    ///
    /// Lowest precedence — overridden by `RUST_LOG` or
    /// [`log_filter`](App::log_filter).
    #[cfg(feature = "logging")]
    pub fn log_level(mut self, level: impl Into<String>) -> Self {
        self.logging_config.level = Some(level.into());
        self
    }

    /// Set a tracing filter directive (e.g., `"my_app=debug,sqlx=warn"`).
    ///
    /// Highest precedence — overrides both [`log_level`](App::log_level)
    /// and the `RUST_LOG` environment variable.
    #[cfg(feature = "logging")]
    pub fn log_filter(mut self, filter: impl Into<String>) -> Self {
        self.logging_config.filter = Some(filter.into());
        self
    }

    /// Disable automatic request logging and request ID middleware.
    ///
    /// The tracing subscriber is still initialized — `tracing::info!()`
    /// and similar calls in your code still work. Only the automatic
    /// per-request logging is turned off.
    #[cfg(feature = "logging")]
    pub fn disable_request_logging(mut self) -> Self {
        self.logging_config.request_logging = false;
        self
    }

    /// Set a custom request ID header name.
    ///
    /// Default: `"x-request-id"`. The middleware reads this header from
    /// incoming requests and writes it on responses.
    #[cfg(feature = "logging")]
    pub fn request_id_header(mut self, header: impl Into<String>) -> Self {
        self.logging_config.request_id_header = header.into();
        self
    }

    /// Set the graceful shutdown timeout.
    ///
    /// After a shutdown signal is received, in-flight requests are given
    /// this long to complete before the server forcibly closes them. The
    /// default is 30 seconds.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::app::App;
    /// use std::time::Duration;
    ///
    /// let _app = App::new().shutdown_timeout(Duration::from_secs(60));
    /// ```
    pub fn shutdown_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Set the maximum request body size in bytes. Default: 2 MiB (2,097,152 bytes).
    ///
    /// Requests with bodies exceeding this limit receive a `413 Payload Too Large`
    /// response before the handler runs.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::prelude::*;
    ///
    /// let app = App::new()
    ///     .body_limit(10 * 1024 * 1024) // 10 MiB for file uploads
    ///     .post("/upload", |_req: Request| "ok");
    /// ```
    pub fn body_limit(mut self, max_bytes: usize) -> Self {
        self.body_limit = max_bytes;
        self
    }

    /// Configure TLS with the given certificate and key file paths.
    ///
    /// The server will serve HTTPS using rustls with ALPN set to
    /// `["h2", "http/1.1"]` for automatic HTTP/2 negotiation.
    ///
    /// # Panics
    ///
    /// Panics at server start if the files cannot be read or parsed.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use ladoo::prelude::*;
    ///
    /// fn main() {
    ///     App::new()
    ///         .tls("cert.pem", "key.pem")
    ///         .get("/", |_: Request| "Hello TLS")
    ///         .run("0.0.0.0:443");
    /// }
    /// ```
    #[cfg(feature = "tls")]
    pub fn tls(
        mut self,
        cert_path: impl Into<std::path::PathBuf>,
        key_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.tls_config = Some(crate::tls::TlsConfig::new(cert_path, key_path));
        self
    }

    /// Register an async shutdown hook.
    ///
    /// The hook runs after all connections have drained (or the shutdown
    /// timeout has expired) during graceful shutdown. Multiple hooks run
    /// concurrently.
    ///
    /// Plugins use this to clean up external resources (close connection
    /// pools, flush buffers, disconnect from services). Users can also
    /// call it directly.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// App::new()
    ///     .on_shutdown(|| async {
    ///         println!("shutting down");
    ///     })
    ///     .run("0.0.0.0:3000");
    /// ```
    pub fn on_shutdown<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(|| Box::pin(hook())));
        self
    }

    /// Register a plugin.
    ///
    /// The plugin's [`register`](crate::plugin::Plugin::register) method is
    /// called with this `App`, allowing it to add routes, state,
    /// middleware, and shutdown hooks. If a plugin with the same
    /// [`name`](crate::plugin::Plugin::name) is already registered, a
    /// warning is logged and the duplicate is skipped.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// App::new()
    ///     .plugin(HealthPlugin)
    ///     .plugin(MetricsPlugin::new(9090))
    ///     .run("0.0.0.0:3000");
    /// ```
    pub fn plugin(mut self, plugin: impl crate::plugin::Plugin) -> Self {
        let name = plugin.name().to_owned();
        if self.plugin_names.contains(&name) {
            #[cfg(feature = "logging")]
            tracing::warn!("plugin '{name}' already registered, skipping");
            #[cfg(not(feature = "logging"))]
            eprintln!("warning: plugin '{name}' already registered, skipping");
            return self;
        }
        self.plugin_names.push(name);
        plugin.register(self)
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

    /// Create a new application for testing.
    ///
    /// This is identical to [`App::new()`] — it exists to signal intent
    /// and read clearly in test code. The returned `App` supports all the
    /// same builder methods.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ladoo::app::App;
    /// use ladoo::request::Request;
    ///
    /// let _app = App::test().get("/", |_: Request| "hello");
    /// ```
    pub fn test() -> Self {
        Self::new()
    }

    /// Consume the app and return an in-memory test client.
    ///
    /// The returned [`TestClient`](crate::testing::TestClient) routes
    /// requests through the middleware chain and handler without opening
    /// a TCP connection.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// let client = App::test()
    ///     .get("/", |_: Request| "hello")
    ///     .into_client();
    ///
    /// let resp = client.get("/").send().await;
    /// assert_eq!(resp.text(), "hello");
    /// ```
    pub fn into_client(self) -> crate::testing::TestClient {
        #[cfg(not(feature = "tls"))]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, _body_limit) =
            self.into_parts();
        #[cfg(feature = "tls")]
        let (router, state, middleware, _shutdown_timeout, _shutdown_hooks, _body_limit, _tls) =
            self.into_parts();
        crate::testing::TestClient::new(router, build_and_initialize_state(state), middleware)
    }

    /// Start a real TCP server on a random port for integration testing.
    ///
    /// The returned [`TestServer`](crate::testing::TestServer) sends
    /// requests over the network and stops the server when dropped.
    ///
    /// Requires the `test-server` feature in downstream crates:
    ///
    /// ```toml
    /// [dev-dependencies]
    /// ladoo = { version = "0.1", features = ["test-server"] }
    /// ```
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// #[tokio::test]
    /// async fn integration() {
    ///     let server = App::test()
    ///         .get("/", |_: Request| "hello")
    ///         .spawn()
    ///         .await;
    ///
    ///     let resp = server.get("/").send().await;
    ///     assert_eq!(resp.text(), "hello");
    /// }
    /// ```
    #[cfg(any(test, feature = "test-server"))]
    pub async fn spawn(self) -> crate::testing::TestServer {
        #[cfg(not(feature = "tls"))]
        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit) =
            self.into_parts();
        #[cfg(feature = "tls")]
        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, _tls) =
            self.into_parts();
        crate::testing::TestServer::start(
            router,
            build_and_initialize_state(state),
            middleware,
            shutdown_timeout,
            shutdown_hooks,
            body_limit,
        )
        .await
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

    /// Consume the App and return the inner router, application state,
    /// global middleware stack, shutdown timeout, shutdown hooks, and
    /// body limit.
    ///
    /// Used internally by the server to access routes, dependency
    /// injection state, middleware, shutdown configuration, and the
    /// request body size limit together.
    #[cfg(not(feature = "tls"))]
    #[allow(clippy::type_complexity)]
    pub(crate) fn into_parts(
        mut self,
    ) -> (
        Router,
        TypeMap,
        Vec<Arc<dyn Middleware>>,
        std::time::Duration,
        Vec<ShutdownHook>,
        usize,
    ) {
        let registry = std::mem::take(&mut self.health_registry);
        let config = std::mem::take(&mut self.health_config);

        if registry.has_checks() {
            let path = config.path.clone();
            self.state.insert_shared(registry);
            self.state.insert_shared(config);
            self.router.add(
                http::Method::GET,
                &path,
                crate::health::health_handler.into_handler(),
            );
        }

        (
            self.router,
            self.state,
            self.global_middleware,
            self.shutdown_timeout,
            self.shutdown_hooks,
            self.body_limit,
        )
    }

    /// Consume the App and return the inner router, application state,
    /// global middleware stack, shutdown timeout, shutdown hooks, body
    /// limit, and TLS configuration.
    ///
    /// Used internally by the server to access routes, dependency
    /// injection state, middleware, shutdown configuration, the request
    /// body size limit, and TLS settings together.
    #[cfg(feature = "tls")]
    #[allow(clippy::type_complexity)]
    pub(crate) fn into_parts(
        mut self,
    ) -> (
        Router,
        TypeMap,
        Vec<Arc<dyn Middleware>>,
        std::time::Duration,
        Vec<ShutdownHook>,
        usize,
        Option<crate::tls::TlsConfig>,
    ) {
        let registry = std::mem::take(&mut self.health_registry);
        let config = std::mem::take(&mut self.health_config);

        if registry.has_checks() {
            let path = config.path.clone();
            self.state.insert_shared(registry);
            self.state.insert_shared(config);
            self.router.add(
                http::Method::GET,
                &path,
                crate::health::health_handler.into_handler(),
            );
        }

        (
            self.router,
            self.state,
            self.global_middleware,
            self.shutdown_timeout,
            self.shutdown_hooks,
            self.body_limit,
            self.tls_config,
        )
    }

    /// Start the HTTP server, blocking the current thread.
    ///
    /// Creates a Tokio runtime internally — no `#[tokio::main]` needed.
    /// This is the simplest way to start a Ladoo app.
    ///
    /// When the `logging` feature is enabled (the default), two built-in
    /// middleware are prepended to the chain before the server starts:
    /// request ID generation and request logging. Call
    /// [`disable_request_logging`](App::disable_request_logging) to turn
    /// this off.
    ///
    /// The server shuts down gracefully on `SIGTERM` or Ctrl-C (`SIGINT`):
    /// it stops accepting new connections and gives in-flight requests up
    /// to [`shutdown_timeout`](App::shutdown_timeout) (default 30 seconds)
    /// to finish before returning.
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
    #[cfg_attr(not(feature = "logging"), allow(unused_mut))]
    pub fn run(mut self, addr: &str) {
        #[cfg(feature = "logging")]
        crate::logging::init_subscriber(&self.logging_config);

        #[cfg(feature = "logging")]
        self.inject_builtin_middleware();

        let rt = tokio::runtime::Runtime::new().expect("failed to create Tokio runtime");

        let addr: std::net::SocketAddr = addr
            .parse()
            .expect("invalid address — expected format like 0.0.0.0:3000");

        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("failed to bind to {addr}: {e}"));

            #[cfg(not(feature = "tls"))]
            let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit) =
                self.into_parts();
            #[cfg(feature = "tls")]
            let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, tls_config) =
                self.into_parts();

            #[cfg(feature = "tls")]
            let tls_acceptor = tls_config.map(|c| c.build_acceptor());

            #[cfg(feature = "tls")]
            if tls_acceptor.is_some() {
                println!("Ladoo listening on https://{addr}");
            } else {
                println!("Ladoo listening on http://{addr}");
            }
            #[cfg(not(feature = "tls"))]
            println!("Ladoo listening on http://{addr}");

            crate::server::serve(
                router,
                listener,
                build_and_initialize_state(state),
                middleware,
                crate::shutdown::shutdown_signal(),
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
                #[cfg(feature = "tls")]
                tls_acceptor,
            )
            .await;
        });
    }

    /// Start the HTTP server using a pre-bound listener.
    ///
    /// Useful for tests (bind to port 0 for a random port) and advanced
    /// use cases where you manage the Tokio runtime yourself.
    ///
    /// When the `logging` feature is enabled (the default), two built-in
    /// middleware are prepended to the chain before the server starts:
    /// request ID generation and request logging. Call
    /// [`disable_request_logging`](App::disable_request_logging) to turn
    /// this off.
    ///
    /// The server shuts down gracefully on `SIGTERM` or Ctrl-C (`SIGINT`):
    /// it stops accepting new connections and gives in-flight requests up
    /// to [`shutdown_timeout`](App::shutdown_timeout) (default 30 seconds)
    /// to finish before returning.
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
    #[cfg_attr(not(feature = "logging"), allow(unused_mut))]
    pub async fn serve_listener(mut self, listener: TcpListener) {
        #[cfg(feature = "logging")]
        crate::logging::init_subscriber(&self.logging_config);

        #[cfg(feature = "logging")]
        self.inject_builtin_middleware();

        #[cfg(not(feature = "tls"))]
        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit) =
            self.into_parts();
        #[cfg(feature = "tls")]
        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, tls_config) =
            self.into_parts();

        #[cfg(feature = "tls")]
        let tls_acceptor = tls_config.map(|c| c.build_acceptor());

        crate::server::serve(
            router,
            listener,
            build_and_initialize_state(state),
            middleware,
            crate::shutdown::shutdown_signal(),
            shutdown_timeout,
            shutdown_hooks,
            body_limit,
            #[cfg(feature = "tls")]
            tls_acceptor,
        )
        .await;
    }

    /// Prepend the built-in request ID and request logger middleware to
    /// the global middleware stack, when request logging is enabled.
    ///
    /// Called by [`App::run`] and [`App::serve_listener`] only — the
    /// in-memory test helpers ([`App::into_client`] and [`App::spawn`])
    /// skip this so tests control their own middleware stack.
    #[cfg(feature = "logging")]
    fn inject_builtin_middleware(&mut self) {
        if !self.logging_config.request_logging {
            return;
        }

        let header = std::mem::take(&mut self.logging_config.request_id_header);
        let mut built_in: Vec<Arc<dyn Middleware>> = vec![
            Arc::new(crate::logging::RequestIdMiddleware::new(header)),
            Arc::new(
                crate::logging::request_logger
                    as fn(crate::context::Context, crate::middleware::Next) -> _,
            ),
        ];
        built_in.append(&mut self.global_middleware);
        self.global_middleware = built_in;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap the final `TypeMap` in an `Arc` and initialize the [`JobRunner`](crate::job::JobRunner),
/// if one was provided via [`App::provide`].
///
/// `into_parts()` returns state as an owned `TypeMap` because plugins and
/// health-check registration still need to mutate it. The `JobRunner`
/// needs a *shared* reference to the finalized state (so jobs can access
/// `State<T>` the same way handlers do) — that reference can only exist
/// once the map is behind an `Arc`. This function is the single point
/// where that happens, called by every path that turns app state into a
/// running server or test client.
#[cfg(feature = "jobs")]
pub(crate) fn build_and_initialize_state(state: TypeMap) -> Arc<TypeMap> {
    let state = Arc::new(state);
    if let Some(runner) = state.get_shared::<crate::job::JobRunner>() {
        runner.initialize(state.clone());
    }
    state
}

/// Wrap the final `TypeMap` in an `Arc`.
///
/// Mirrors the `jobs`-enabled version above so call sites don't need to
/// branch on the feature flag.
#[cfg(not(feature = "jobs"))]
pub(crate) fn build_and_initialize_state(state: TypeMap) -> Arc<TypeMap> {
    Arc::new(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FromRequest;
    use crate::request::Request;
    use http::Method;

    /// Test-only helper that normalizes `into_parts()` to the 6-tuple
    /// shape regardless of whether the `tls` feature is enabled, so the
    /// bulk of these tests don't need to branch on the feature flag.
    #[allow(clippy::type_complexity)]
    fn test_into_parts(
        app: App,
    ) -> (
        Router,
        TypeMap,
        Vec<Arc<dyn Middleware>>,
        std::time::Duration,
        Vec<ShutdownHook>,
        usize,
    ) {
        #[cfg(not(feature = "tls"))]
        {
            app.into_parts()
        }
        #[cfg(feature = "tls")]
        {
            let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, _tls) =
                app.into_parts();
            (
                router,
                state,
                middleware,
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
            )
        }
    }

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
            .provide(2.72_f64);
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
        let (router, state, _middleware, _shutdown_timeout, _shutdown_hooks, _body_limit) =
            test_into_parts(app);
        assert!(router.find(&Method::GET, "/").is_some());
        assert_eq!(*state.get_shared::<u32>().unwrap(), 42);
    }

    #[cfg(feature = "json")]
    #[test]
    fn pagination_stores_config() {
        let app = App::new().pagination(
            crate::pagination::PaginationConfig::new()
                .default_per_page(25)
                .max_per_page(50),
        );
        let (_, state, _, _, _, _) = test_into_parts(app);
        let config = state
            .get_shared::<crate::pagination::PaginationConfig>()
            .unwrap();
        assert_eq!(config.default_per_page, 25);
        assert_eq!(config.max_per_page, 50);
    }

    #[test]
    fn shutdown_timeout_stores_duration() {
        let app = App::new().shutdown_timeout(std::time::Duration::from_secs(60));
        let (_, _, _, timeout, _, _) = test_into_parts(app);
        assert_eq!(timeout, std::time::Duration::from_secs(60));
    }

    #[test]
    fn default_shutdown_timeout_is_30s() {
        let app = App::new();
        let (_, _, _, timeout, _, _) = test_into_parts(app);
        assert_eq!(timeout, std::time::Duration::from_secs(30));
    }

    #[test]
    fn body_limit_stores_value() {
        let app = App::new().body_limit(1024);
        let (_, _, _, _, _, body_limit) = test_into_parts(app);
        assert_eq!(body_limit, 1024);
    }

    #[test]
    fn default_body_limit_is_2mib() {
        let app = App::new();
        let (_, _, _, _, _, body_limit) = test_into_parts(app);
        assert_eq!(body_limit, 2_097_152);
    }

    #[test]
    fn on_shutdown_stores_hooks() {
        let app = App::new()
            .on_shutdown(|| async {})
            .on_shutdown(|| async {});
        let (_, _, _, _, hooks, _) = test_into_parts(app);
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn use_mw_chains() {
        async fn noop(ctx: crate::context::Context, next: crate::middleware::Next) -> crate::error::Result<crate::response::Response> {
            next.run(ctx).await
        }
        let app = App::new()
            .use_mw(noop)
            .get("/", |_req: Request| "hello");
        let _ = test_into_parts(app);
    }

    #[test]
    fn group_adds_prefixed_routes() {
        let app = App::new().group("/api", |r| {
            r.get("/users", |_req: Request| "users")
                .post("/users", |_req: Request| "created")
        });
        let (router, _, _, _, _, _) = test_into_parts(app);
        assert!(router.find(&Method::GET, "/api/users").is_some());
        assert!(router.find(&Method::POST, "/api/users").is_some());
    }

    #[test]
    fn mount_adds_prefixed_routes() {
        let api = Router::new().get("/items", |_req: Request| "items");
        let app = App::new().mount("/api", api);
        let (router, _, _, _, _, _) = test_into_parts(app);
        assert!(router.find(&Method::GET, "/api/items").is_some());
    }

    #[cfg(feature = "logging")]
    #[test]
    fn log_level_sets_config() {
        let app = App::new().log_level("debug");
        assert_eq!(app.logging_config.level, Some("debug".to_string()));
    }

    #[cfg(feature = "logging")]
    #[test]
    fn log_filter_sets_config() {
        let app = App::new().log_filter("my_app=debug,sqlx=warn");
        assert_eq!(
            app.logging_config.filter,
            Some("my_app=debug,sqlx=warn".to_string())
        );
    }

    #[cfg(feature = "logging")]
    #[test]
    fn disable_request_logging_sets_config() {
        let app = App::new().disable_request_logging();
        assert!(!app.logging_config.request_logging);
    }

    #[cfg(feature = "logging")]
    #[test]
    fn request_id_header_sets_config() {
        let app = App::new().request_id_header("x-trace-id");
        assert_eq!(app.logging_config.request_id_header, "x-trace-id");
    }

    #[cfg(feature = "logging")]
    #[test]
    fn log_level_default_is_none() {
        let app = App::new();
        assert!(app.logging_config.level.is_none());
    }

    #[cfg(feature = "logging")]
    #[tokio::test]
    async fn auto_injects_request_id_middleware() {
        let app = App::test().get("/", |_req: crate::request::Request| "hello");
        let client = app.into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.status(), 200);
        // into_client does NOT auto-inject — this tests the negative case
        assert!(resp.header("x-request-id").is_none());
    }

    #[cfg(feature = "logging")]
    #[tokio::test]
    async fn disable_request_logging_skips_middleware() {
        let app = App::test()
            .disable_request_logging()
            .get("/", |_req: crate::request::Request| "hello");
        let (_, _, middleware, _, _, _) = test_into_parts(app);
        // No built-in middleware when disabled
        assert!(middleware.is_empty());
    }

    #[cfg(feature = "logging")]
    #[test]
    fn inject_builtin_middleware_prepends_request_id_and_logger() {
        async fn noop(
            ctx: crate::context::Context,
            next: crate::middleware::Next,
        ) -> crate::error::Result<crate::response::Response> {
            next.run(ctx).await
        }
        let mut app = App::new().use_mw(noop);
        assert_eq!(app.global_middleware.len(), 1);
        app.inject_builtin_middleware();
        // Two built-in middleware (request ID + logger) prepended before
        // the user's `noop` middleware.
        assert_eq!(app.global_middleware.len(), 3);
    }

    #[cfg(feature = "logging")]
    #[test]
    fn inject_builtin_middleware_is_noop_when_disabled() {
        let mut app = App::new().disable_request_logging();
        assert!(app.global_middleware.is_empty());
        app.inject_builtin_middleware();
        assert!(app.global_middleware.is_empty());
    }

    mod plugin_tests {
        use super::*;

        struct GreetPlugin;

        impl crate::plugin::Plugin for GreetPlugin {
            fn name(&self) -> &str {
                "greet"
            }

            fn register(self, app: App) -> App {
                app.provide("hello".to_string())
                    .get("/greet", |_req: Request| "hi")
            }
        }

        #[test]
        fn plugin_registers_state_and_route() {
            let app = App::new().plugin(GreetPlugin);
            let (router, state, _, _, _, _) = test_into_parts(app);
            assert_eq!(*state.get_shared::<String>().unwrap(), "hello".to_string());
            assert!(router.find(&Method::GET, "/greet").is_some());
        }

        struct DuplicatePlugin(u32);

        impl crate::plugin::Plugin for DuplicatePlugin {
            fn name(&self) -> &str {
                "dup"
            }

            fn register(self, app: App) -> App {
                app.provide(self.0)
            }
        }

        #[test]
        fn duplicate_plugin_skipped() {
            let app = App::new()
                .plugin(DuplicatePlugin(1))
                .plugin(DuplicatePlugin(2));
            let (_, state, _, _, _, _) = test_into_parts(app);
            assert_eq!(*state.get_shared::<u32>().unwrap(), 1);
        }

        struct SubPluginParent;
        struct SubPluginChild;

        impl crate::plugin::Plugin for SubPluginChild {
            fn name(&self) -> &str {
                "child"
            }

            fn register(self, app: App) -> App {
                app.provide(99_u32)
            }
        }

        impl crate::plugin::Plugin for SubPluginParent {
            fn name(&self) -> &str {
                "parent"
            }

            fn register(self, app: App) -> App {
                app.plugin(SubPluginChild)
            }
        }

        #[test]
        fn sub_plugin_registers_via_parent() {
            let app = App::new().plugin(SubPluginParent);
            let (_, state, _, _, _, _) = test_into_parts(app);
            assert_eq!(*state.get_shared::<u32>().unwrap(), 99);
        }

        #[test]
        fn plugin_chains_with_routes() {
            let app = App::new()
                .get("/before", |_req: Request| "before")
                .plugin(GreetPlugin)
                .get("/after", |_req: Request| "after");
            let (router, _, _, _, _, _) = test_into_parts(app);
            assert!(router.find(&Method::GET, "/before").is_some());
            assert!(router.find(&Method::GET, "/greet").is_some());
            assert!(router.find(&Method::GET, "/after").is_some());
        }
    }

    mod health_tests {
        use super::*;
        use crate::health::{HealthCheckable, HealthConfig};

        #[derive(Clone)]
        struct MockDb;

        #[async_trait::async_trait]
        impl HealthCheckable for MockDb {
            fn name(&self) -> &str {
                "mock-db"
            }
            async fn check(&self) -> crate::error::Result<()> {
                Ok(())
            }
        }

        #[test]
        fn provide_healthy_stores_state_and_registry() {
            let app = App::new().provide_healthy(MockDb);
            let (_, state, _, _, _, _) = test_into_parts(app);
            assert!(state.get_shared::<MockDb>().is_some());
        }

        #[test]
        fn health_closure_registers_check() {
            let app = App::new().health("test", || async { Ok(()) });
            // Verify by checking that into_parts produces a route
            let (router, _, _, _, _, _) = test_into_parts(app);
            assert!(router.find(&Method::GET, "/health").is_some());
        }

        #[test]
        fn health_config_sets_custom_path() {
            let app = App::new()
                .health("test", || async { Ok(()) })
                .health_config(HealthConfig::new().path("/healthz"));
            let (router, _, _, _, _, _) = test_into_parts(app);
            assert!(router.find(&Method::GET, "/healthz").is_some());
            assert!(router.find(&Method::GET, "/health").is_none());
        }

        #[test]
        fn no_health_checks_no_route() {
            let app = App::new();
            let (router, _, _, _, _, _) = test_into_parts(app);
            assert!(router.find(&Method::GET, "/health").is_none());
        }

        #[tokio::test]
        async fn health_endpoint_via_client() {
            let client = App::test().provide_healthy(MockDb).into_client();
            let resp = client.get("/health").send().await;
            assert_eq!(resp.status(), 200);
            let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
            assert_eq!(body["status"], "healthy");
            assert_eq!(body["checks"]["mock-db"]["status"], "up");
        }
    }

    #[cfg(feature = "config")]
    mod config_tests {
        use super::*;
        use crate::config::{Config, ConfigError};

        struct TestConfig {
            port: u16,
        }

        impl Config for TestConfig {
            fn load() -> std::result::Result<Self, ConfigError> {
                Ok(TestConfig { port: 9090 })
            }
        }

        #[test]
        fn config_provides_as_state() {
            let app = App::new().config::<TestConfig>();
            let (_, state, _, _, _, _) = test_into_parts(app);
            assert_eq!(state.get_shared::<TestConfig>().unwrap().port, 9090);
        }

        struct FailConfig;
        impl Config for FailConfig {
            fn load() -> std::result::Result<Self, ConfigError> {
                Err(ConfigError::MissingField {
                    field: "required",
                    expected_type: "String",
                })
            }
        }

        #[test]
        #[should_panic(expected = "configuration error")]
        fn config_panics_on_error() {
            let _app = App::new().config::<FailConfig>();
        }

        #[test]
        fn config_chains_with_routes() {
            let app = App::new()
                .config::<TestConfig>()
                .get("/", |_req: Request| "hello");
            let (router, state, _, _, _, _) = test_into_parts(app);
            assert!(router.find(&Method::GET, "/").is_some());
            assert_eq!(state.get_shared::<TestConfig>().unwrap().port, 9090);
        }
    }
}
