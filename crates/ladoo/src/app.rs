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

use crate::handler::IntoHandler;
use crate::router::Router;

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
}

impl App {
    /// Create a new application with no routes.
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
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

    /// Consume the App and return the inner router.
    ///
    /// Used internally by the server to access routes.
    #[allow(dead_code)]
    pub(crate) fn into_router(self) -> Router {
        self.router
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
}
