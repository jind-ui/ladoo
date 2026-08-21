//! Job registry — maps job names to deserializer + handler functions.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// A persistent background job that can be serialized to a database.
///
/// This is the Mode 2 equivalent of `ladoo::Job`. Implementors must
/// also derive `Serialize` and `Deserialize` so the job data can
/// round-trip through the database.
pub trait PersistentJob: Send + Sync + 'static + Serialize + DeserializeOwned {
    /// Unique name for this job type. Must be stable across deploys.
    fn name() -> &'static str;

    /// Maximum number of retries (default: 3).
    fn max_retries() -> u32 {
        3
    }

    /// Execute the job.
    fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

type BoxedHandler = Box<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

struct JobFactory {
    handler: BoxedHandler,
    #[allow(dead_code)] // will be consulted by the worker once retry policy is wired up
    max_retries: u32,
}

/// Registry mapping job names to their handlers.
///
/// The worker uses this to deserialize a `QueuedJob`'s payload and
/// dispatch it to the correct handler function.
pub struct JobRegistry {
    handlers: HashMap<String, JobFactory>,
}

impl JobRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a job type. The worker will be able to handle jobs
    /// whose `name` matches `J::name()`.
    pub fn register<J: PersistentJob>(&mut self) -> &mut Self {
        let factory = JobFactory {
            handler: Box::new(|payload: serde_json::Value| {
                Box::pin(async move {
                    let job: J = serde_json::from_value(payload)
                        .map_err(|e| format!("deserialization error: {e}"))?;
                    job.handle().await
                })
            }),
            max_retries: J::max_retries(),
        };
        self.handlers.insert(J::name().to_string(), factory);
        self
    }

    /// Look up a handler by job name and execute it with the given payload.
    pub(crate) async fn dispatch(
        &self,
        name: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        let factory = self
            .handlers
            .get(name)
            .ok_or_else(|| format!("no handler registered for job '{name}'"))?;
        (factory.handler)(payload).await
    }

    /// Check if a handler is registered for the given name.
    pub fn has(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct TestJob {
        value: String,
    }

    impl PersistentJob for TestJob {
        fn name() -> &'static str {
            "test_job"
        }
        fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct FailingJob;

    impl PersistentJob for FailingJob {
        fn name() -> &'static str {
            "failing_job"
        }
        fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("intentional failure".into()) })
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct CustomRetriesJob;

    impl PersistentJob for CustomRetriesJob {
        fn name() -> &'static str {
            "custom_retries_job"
        }
        fn max_retries() -> u32 {
            7
        }
        fn handle(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn register_and_has() {
        let mut registry = JobRegistry::new();
        assert!(!registry.has("test_job"));
        registry.register::<TestJob>();
        assert!(registry.has("test_job"));
    }

    #[test]
    fn default_impl_is_empty() {
        let registry = JobRegistry::default();
        assert!(!registry.has("anything"));
    }

    #[test]
    fn default_max_retries_is_three() {
        assert_eq!(TestJob::max_retries(), 3);
    }

    #[test]
    fn custom_max_retries_is_honored() {
        assert_eq!(CustomRetriesJob::max_retries(), 7);
    }

    #[tokio::test]
    async fn dispatch_success() {
        let mut registry = JobRegistry::new();
        registry.register::<TestJob>();
        let payload = serde_json::json!({"value": "hello"});
        let result = registry.dispatch("test_job", payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn dispatch_failure() {
        let mut registry = JobRegistry::new();
        registry.register::<FailingJob>();
        let result = registry
            .dispatch("failing_job", serde_json::json!(null))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("intentional failure"));
    }

    #[tokio::test]
    async fn dispatch_unknown_job() {
        let registry = JobRegistry::new();
        let result = registry
            .dispatch("nonexistent", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no handler registered"));
    }

    #[tokio::test]
    async fn dispatch_bad_payload() {
        let mut registry = JobRegistry::new();
        registry.register::<TestJob>();
        // TestJob expects {"value": "..."} but we send a number
        let result = registry.dispatch("test_job", serde_json::json!(42)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("deserialization"));
    }

    #[test]
    fn register_returns_mut_ref_for_chaining() {
        let mut registry = JobRegistry::new();
        registry.register::<TestJob>().register::<FailingJob>();
        assert!(registry.has("test_job"));
        assert!(registry.has("failing_job"));
    }
}
