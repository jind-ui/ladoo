//! Integration tests running the conformance suite against all drivers.

#[cfg(feature = "sqlite")]
mod sqlite_conformance {
    use ladoo_migrate::driver::sqlite::SqliteDriver;
    use ladoo_migrate::testing::driver_conformance;

    #[tokio::test]
    async fn sqlite_passes_conformance_suite() {
        driver_conformance::run_all::<SqliteDriver>("sqlite::memory:").await;
    }
}

#[cfg(feature = "test-postgres")]
mod postgres_conformance {
    use ladoo_migrate::driver::postgres::PostgresDriver;
    use ladoo_migrate::testing::driver_conformance;

    #[tokio::test]
    async fn postgres_passes_conformance_suite() {
        let url = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/ladoo_test".into());
        driver_conformance::run_all::<PostgresDriver>(&url).await;
    }
}

#[cfg(feature = "test-mysql")]
mod mysql_conformance {
    use ladoo_migrate::driver::mysql::MysqlDriver;
    use ladoo_migrate::testing::driver_conformance;

    #[tokio::test]
    async fn mysql_passes_conformance_suite() {
        let url = std::env::var("TEST_MYSQL_URL")
            .unwrap_or_else(|_| "mysql://root:root@localhost/ladoo_test".into());
        driver_conformance::run_all::<MysqlDriver>(&url).await;
    }
}
