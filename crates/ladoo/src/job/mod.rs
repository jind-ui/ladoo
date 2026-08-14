//! In-process job execution.
//!
//! Define jobs as structs implementing [`Job`], then dispatch them via
//! [`JobRunner`]. Jobs run as `tokio::spawn` tasks with configurable
//! retries, timeout, and backoff.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//!
//! #[derive(Job)]
//! #[job(retries = 3, timeout = "30s")]
//! struct SendEmail { user_id: i64 }
//!
//! impl SendEmail {
//!     async fn handle(&self, ctx: &JobContext) -> Result<(), JobError> {
//!         let mailer = ctx.state::<Mailer>()?;
//!         mailer.send(self.user_id).await.map_err(JobError::failed)
//!     }
//! }
//! ```

mod context;
mod error;
pub(crate) mod runner;

pub use context::JobContext;
pub use error::JobError;
pub use runner::{JobHandle, JobRunner};

use std::future::Future;
use std::time::Duration;

/// A background job that can be dispatched by [`JobRunner`].
///
/// Implement this trait directly, or use `#[derive(Job)]` to generate
/// `name()` and `config()` from struct attributes.
///
/// The `handle` method receives a [`JobContext`] for accessing
/// application state (the same values registered with `App::provide()`).
pub trait Job: Send + Sync + 'static {
    /// Unique name for this job type.
    fn name(&self) -> &'static str;

    /// Job configuration — retries, timeout, backoff.
    fn config(&self) -> JobConfig {
        JobConfig::default()
    }

    /// Execute the job.
    fn handle(&self, ctx: &JobContext) -> impl Future<Output = Result<(), JobError>> + Send;
}

/// Runtime configuration for a job.
///
/// Controls retry count, timeout per attempt, and backoff strategy
/// between retries.
#[derive(Debug, Clone)]
pub struct JobConfig {
    /// Maximum number of retries after the first attempt fails (default: 0).
    pub max_retries: u32,
    /// Maximum duration for a single attempt (default: 30s).
    pub timeout: Duration,
    /// Delay strategy between retries (default: exponential, 1s base, 60s max).
    pub backoff: BackoffStrategy,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            max_retries: 0,
            timeout: Duration::from_secs(30),
            backoff: BackoffStrategy::exponential_default(),
        }
    }
}

/// Strategy for computing the delay between job retries.
#[derive(Debug, Clone)]
pub enum BackoffStrategy {
    /// Wait a fixed duration between every retry.
    Fixed(Duration),
    /// Exponential backoff: `base * 2^retry`, capped at `max`.
    Exponential {
        /// Initial delay (doubles each retry).
        base: Duration,
        /// Maximum delay cap.
        max: Duration,
    },
}

impl BackoffStrategy {
    /// Default exponential backoff: 1 second base, 60 second cap.
    pub fn exponential_default() -> Self {
        Self::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
        }
    }

    /// Compute the delay for a given retry number (0-indexed).
    ///
    /// Retry 0 = first retry after the initial attempt.
    pub fn delay(&self, retry: u32) -> Duration {
        match self {
            Self::Fixed(d) => *d,
            Self::Exponential { base, max } => {
                let delay = base.saturating_mul(2u32.saturating_pow(retry));
                std::cmp::min(delay, *max)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = JobConfig::default();
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert!(matches!(
            config.backoff,
            BackoffStrategy::Exponential { .. }
        ));
    }

    #[test]
    fn fixed_backoff_constant_delay() {
        let strategy = BackoffStrategy::Fixed(Duration::from_millis(500));
        assert_eq!(strategy.delay(0), Duration::from_millis(500));
        assert_eq!(strategy.delay(1), Duration::from_millis(500));
        assert_eq!(strategy.delay(10), Duration::from_millis(500));
    }

    #[test]
    fn exponential_backoff_doubles() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
        };
        assert_eq!(strategy.delay(0), Duration::from_secs(1));
        assert_eq!(strategy.delay(1), Duration::from_secs(2));
        assert_eq!(strategy.delay(2), Duration::from_secs(4));
        assert_eq!(strategy.delay(3), Duration::from_secs(8));
    }

    #[test]
    fn exponential_backoff_caps_at_max() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(10),
        };
        assert_eq!(strategy.delay(5), Duration::from_secs(10));
        assert_eq!(strategy.delay(20), Duration::from_secs(10));
    }

    #[test]
    fn exponential_backoff_overflow_saturates() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
        };
        // 2^31 would overflow u32 — saturating_pow prevents panic
        assert!(strategy.delay(31) <= Duration::from_secs(60));
    }

    #[test]
    fn exponential_default_values() {
        let strategy = BackoffStrategy::exponential_default();
        match strategy {
            BackoffStrategy::Exponential { base, max } => {
                assert_eq!(base, Duration::from_secs(1));
                assert_eq!(max, Duration::from_secs(60));
            }
            _ => panic!("expected Exponential"),
        }
    }
}
