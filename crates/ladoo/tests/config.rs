//! Integration tests for #[derive(Config)].

use std::sync::Mutex;

use ladoo::config::{Config, ConfigError};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(ladoo::Config)]
struct BasicConfig {
    #[config(default = 3000)]
    port: u16,
    #[config(default = "localhost")]
    host: String,
}

#[test]
fn derive_config_uses_defaults() {
    let config = BasicConfig::load().unwrap();
    assert_eq!(config.port, 3000);
    assert_eq!(config.host, "localhost");
}

#[derive(Debug, ladoo::Config)]
struct EnvConfig {
    #[config(env = "TEST_LADOO_PORT", default = 3000)]
    port: u16,
}

#[test]
fn derive_config_reads_env_var() {
    let _g = lock_env();
    std::env::set_var("TEST_LADOO_PORT", "9090");
    let config = EnvConfig::load().unwrap();
    assert_eq!(config.port, 9090);
    std::env::remove_var("TEST_LADOO_PORT");
}

#[test]
fn derive_config_env_var_parse_error() {
    let _g = lock_env();
    std::env::set_var("TEST_LADOO_PORT", "not_a_number");
    let err = EnvConfig::load().unwrap_err();
    assert!(matches!(err, ConfigError::EnvVarParse { .. }));
    std::env::remove_var("TEST_LADOO_PORT");
}

#[test]
fn derive_config_falls_back_to_default() {
    let _g = lock_env();
    std::env::remove_var("TEST_LADOO_PORT");
    let config = EnvConfig::load().unwrap();
    assert_eq!(config.port, 3000);
}

#[derive(ladoo::Config)]
struct OptionalConfig {
    pool_size: Option<u32>,
    #[config(default = 3000)]
    port: u16,
}

#[test]
fn derive_config_option_none_when_missing() {
    let config = OptionalConfig::load().unwrap();
    assert_eq!(config.pool_size, None);
    assert_eq!(config.port, 3000);
}

#[derive(ladoo::Config)]
struct OptionalEnvConfig {
    #[config(env = "TEST_LADOO_POOL")]
    pool_size: Option<u32>,
}

#[test]
fn derive_config_option_some_from_env() {
    let _g = lock_env();
    std::env::set_var("TEST_LADOO_POOL", "10");
    let config = OptionalEnvConfig::load().unwrap();
    assert_eq!(config.pool_size, Some(10));
    std::env::remove_var("TEST_LADOO_POOL");
}

#[derive(Debug, ladoo::Config)]
struct RequiredConfig {
    #[allow(dead_code)]
    database_url: String,
}

#[test]
fn derive_config_missing_required_field_errors() {
    let err = RequiredConfig::load().unwrap_err();
    assert!(matches!(
        err,
        ConfigError::MissingField {
            field: "database_url",
            ..
        }
    ));
}

#[derive(ladoo::Config)]
struct BoolConfig {
    #[config(default = true)]
    debug: bool,
}

#[test]
fn derive_config_bool_default() {
    let config = BoolConfig::load().unwrap();
    assert!(config.debug);
}

#[derive(ladoo::Config)]
struct MultiFieldConfig {
    #[config(default = 3000)]
    port: u16,
    #[config(env = "TEST_LADOO_DB_URL", default = "sqlite::memory:")]
    database_url: String,
    #[config(default = 5)]
    pool_size: u32,
    debug: Option<bool>,
}

#[test]
fn derive_config_multi_field() {
    let _g = lock_env();
    std::env::remove_var("TEST_LADOO_DB_URL");
    let config = MultiFieldConfig::load().unwrap();
    assert_eq!(config.port, 3000);
    assert_eq!(config.database_url, "sqlite::memory:");
    assert_eq!(config.pool_size, 5);
    assert_eq!(config.debug, None);
}

#[test]
fn derive_config_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BasicConfig>();
}
