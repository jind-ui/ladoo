use ladoo::prelude::*;

struct HealthPlugin;

impl Plugin for HealthPlugin {
    fn name(&self) -> &str {
        "health"
    }

    fn register(self, app: App) -> App {
        app.provide("healthy".to_string())
            .get("/health", |status: State<String>| {
                format!("status: {}", *status)
            })
    }
}

#[tokio::test]
async fn plugin_via_prelude_adds_route_and_state() {
    let client = App::test()
        .plugin(HealthPlugin)
        .into_client();

    let resp = client.get("/health").send().await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text(), "status: healthy");
}

#[tokio::test]
async fn plugin_coexists_with_direct_routes() {
    let client = App::test()
        .get("/", |_: Request| "home")
        .plugin(HealthPlugin)
        .get("/about", |_: Request| "about")
        .into_client();

    let home = client.get("/").send().await;
    assert_eq!(home.text(), "home");

    let health = client.get("/health").send().await;
    assert_eq!(health.text(), "status: healthy");

    let about = client.get("/about").send().await;
    assert_eq!(about.text(), "about");
}

#[tokio::test]
async fn duplicate_plugin_uses_first_registration() {
    struct CountPlugin(u32);

    impl Plugin for CountPlugin {
        fn name(&self) -> &str {
            "count"
        }

        fn register(self, app: App) -> App {
            app.provide(self.0)
                .get("/count", |n: State<u32>| format!("{}", *n))
        }
    }

    let client = App::test()
        .plugin(CountPlugin(1))
        .plugin(CountPlugin(2))
        .into_client();

    let resp = client.get("/count").send().await;
    assert_eq!(resp.text(), "1");
}
