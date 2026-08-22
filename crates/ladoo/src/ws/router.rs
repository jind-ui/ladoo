//! Channel topic router.

use std::sync::Arc;

use super::channel::Channel;

/// Maps topic patterns to [`Channel`] implementations.
///
/// Patterns support exact match and wildcard suffix:
/// - `"chat:*"` matches any topic starting with `"chat:"`
/// - `"system:alerts"` matches only `"system:alerts"` exactly
///
/// Routes are matched in registration order, so when multiple patterns
/// could match the same topic, the first one registered wins.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::ws::ChannelRouter;
///
/// let router = ChannelRouter::new()
///     .route("chat:*", ChatChannel)
///     .route("game:*", GameChannel)
///     .route("system:status", StatusChannel);
/// ```
pub struct ChannelRouter {
    routes: Vec<(TopicPattern, Arc<dyn Channel>)>,
}

enum TopicPattern {
    Exact(String),
    Wildcard(String),
}

impl ChannelRouter {
    /// Create an empty channel router.
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Register a [`Channel`] for a topic pattern.
    ///
    /// A pattern ending in `:*` matches any topic sharing that prefix
    /// (e.g. `"chat:*"` matches `"chat:lobby"`); any other pattern must
    /// match the topic exactly.
    pub fn route(mut self, pattern: &str, channel: impl Channel) -> Self {
        let pat = if let Some(prefix) = pattern.strip_suffix('*') {
            TopicPattern::Wildcard(prefix.to_string())
        } else {
            TopicPattern::Exact(pattern.to_string())
        };
        self.routes.push((pat, Arc::new(channel)));
        self
    }

    /// Find the [`Channel`] matching a topic string.
    ///
    /// Routes are checked in registration order and the first match is
    /// returned.
    pub(crate) fn find(&self, topic: &str) -> Option<&Arc<dyn Channel>> {
        for (pattern, channel) in &self.routes {
            match pattern {
                TopicPattern::Exact(s) if s == topic => {
                    return Some(channel);
                }
                TopicPattern::Wildcard(prefix) if topic.starts_with(prefix.as_str()) => {
                    return Some(channel);
                }
                _ => {}
            }
        }
        None
    }
}

impl Default for ChannelRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyChannel;

    #[async_trait::async_trait]
    impl crate::ws::channel::Channel for DummyChannel {
        async fn join(
            &self,
            _topic: &str,
            _payload: serde_json::Value,
            _ctx: &crate::ws::channel::ChannelContext,
        ) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!(null))
        }

        async fn handle(
            &self,
            _event: &str,
            _payload: serde_json::Value,
            _ctx: &crate::ws::channel::ChannelContext,
        ) -> Result<crate::ws::channel::Reply, ()> {
            Ok(crate::ws::channel::Reply::None)
        }
    }

    #[test]
    fn exact_match() {
        let router = ChannelRouter::new().route("system:status", DummyChannel);
        assert!(router.find("system:status").is_some());
    }

    #[test]
    fn exact_no_match() {
        let router = ChannelRouter::new().route("system:status", DummyChannel);
        assert!(router.find("system:health").is_none());
    }

    #[test]
    fn wildcard_match() {
        let router = ChannelRouter::new().route("chat:*", DummyChannel);
        assert!(router.find("chat:lobby").is_some());
        assert!(router.find("chat:room-42").is_some());
    }

    #[test]
    fn wildcard_no_match() {
        let router = ChannelRouter::new().route("chat:*", DummyChannel);
        assert!(router.find("game:lobby").is_none());
    }

    #[test]
    fn wildcard_requires_prefix() {
        let router = ChannelRouter::new().route("chat:*", DummyChannel);
        // "chat" without colon should not match "chat:*"
        assert!(router.find("chat").is_none());
    }

    #[test]
    fn multiple_routes() {
        let router = ChannelRouter::new()
            .route("chat:*", DummyChannel)
            .route("game:*", DummyChannel)
            .route("system:status", DummyChannel);
        assert!(router.find("chat:lobby").is_some());
        assert!(router.find("game:room-1").is_some());
        assert!(router.find("system:status").is_some());
        assert!(router.find("other:thing").is_none());
    }

    #[test]
    fn empty_router() {
        let router = ChannelRouter::new();
        assert!(router.find("anything").is_none());
    }
}
