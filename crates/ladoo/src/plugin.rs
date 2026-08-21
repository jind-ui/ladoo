//! Plugin system for composable application configuration.
//!
//! A [`Plugin`] packages routes, state, middleware, and shutdown cleanup
//! into a single reusable component. Register a plugin with
//! [`App::plugin()`](crate::app::App::plugin).
//!
//! Plugins run once at startup — they receive the [`App`](crate::app::App)
//! builder, add whatever they need, and return it. There is no runtime
//! plugin dispatch; plugins have zero per-request cost unless they
//! explicitly register middleware.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! struct HealthPlugin;
//!
//! impl Plugin for HealthPlugin {
//!     fn name(&self) -> &str { "health" }
//!
//!     fn register(self, app: App) -> App {
//!         app.get("/health", |_: Request| "ok")
//!     }
//! }
//!
//! App::new()
//!     .plugin(HealthPlugin)
//!     .run("0.0.0.0:3000");
//! ```

use std::future::Future;
use std::pin::Pin;

use crate::app::App;

/// A boxed async shutdown cleanup function.
///
/// Stored by [`App::on_shutdown`](crate::app::App::on_shutdown) and
/// executed after all connections drain during graceful shutdown.
pub(crate) type ShutdownHook = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A composable unit of application configuration.
///
/// Plugins package routes, state, middleware, and shutdown cleanup into
/// a single reusable component. Register a plugin with
/// [`App::plugin()`](crate::app::App::plugin).
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::prelude::*;
///
/// struct HealthPlugin;
///
/// impl Plugin for HealthPlugin {
///     fn name(&self) -> &str { "health" }
///
///     fn register(self, app: App) -> App {
///         app.get("/health", |_: Request| "ok")
///     }
/// }
///
/// App::new()
///     .plugin(HealthPlugin)
///     .run("0.0.0.0:3000");
/// ```
pub trait Plugin: Send + Sync + 'static {
    /// Unique name for this plugin.
    ///
    /// Used for duplicate detection and logging. If two plugins share a
    /// name, the second registration is skipped with a warning.
    fn name(&self) -> &str;

    /// Configure the application.
    ///
    /// Called once during startup. The plugin receives the `App` builder
    /// and returns it after adding routes, state, middleware, or shutdown
    /// hooks. The plugin is consumed — all setup happens here.
    fn register(self, app: App) -> App;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::request::Request;

    struct TestPlugin {
        greeting: String,
    }

    impl Plugin for TestPlugin {
        fn name(&self) -> &str {
            "test"
        }

        fn register(self, app: App) -> App {
            app.provide(self.greeting)
        }
    }

    #[test]
    fn plugin_name_returns_str() {
        let plugin = TestPlugin {
            greeting: "hello".into(),
        };
        assert_eq!(plugin.name(), "test");
    }

    #[test]
    fn plugin_register_adds_state() {
        let app = App::new();
        let plugin = TestPlugin {
            greeting: "hi".into(),
        };
        let app = plugin.register(app);
        #[cfg(not(feature = "tls"))]
        let (_, state, _, _, _, _) = app.into_parts();
        #[cfg(feature = "tls")]
        let (_, state, _, _, _, _, _) = app.into_parts();
        assert_eq!(*state.get_shared::<String>().unwrap(), "hi".to_string());
    }

    #[test]
    fn plugin_register_returns_app() {
        let app = App::new().get("/", |_req: Request| "before");
        let plugin = TestPlugin {
            greeting: "hi".into(),
        };
        let app = plugin.register(app);
        let router = app.into_router();
        assert!(router.find(&http::Method::GET, "/").is_some());
    }

    use crate::context::Context;
    use crate::error::Result;
    use crate::middleware::{Middleware, Next};
    use crate::response::Response;
    use std::future::Future;
    use std::pin::Pin;

    struct TagMiddleware;

    impl Middleware for TagMiddleware {
        fn call(
            &self,
            ctx: Context,
            next: Next,
        ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
            Box::pin(async move {
                let mut resp = next.run(ctx).await?;
                resp.set_header("X-Plugin", "tagged");
                Ok(resp)
            })
        }
    }

    struct MiddlewarePlugin;

    impl Plugin for MiddlewarePlugin {
        fn name(&self) -> &str {
            "mw-plugin"
        }

        fn register(self, app: App) -> App {
            app.use_mw(TagMiddleware)
        }
    }

    #[tokio::test]
    async fn plugin_registers_middleware() {
        let client = App::test()
            .plugin(MiddlewarePlugin)
            .get("/", |_req: crate::request::Request| "ok")
            .into_client();
        let resp = client.get("/").send().await;
        assert_eq!(resp.header("X-Plugin"), Some("tagged"));
    }
}
