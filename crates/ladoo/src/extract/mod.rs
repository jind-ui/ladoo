//! Request extractors.
//!
//! Extractors implement [`FromRequest`] to pull typed data out of an
//! HTTP request. Use them as handler arguments to get automatic parsing.
//!
//! # Examples
//!
//! ```
//! use ladoo::extract::FromRequest;
//! use ladoo::request::Request;
//! use http::Method;
//!
//! // Any type implementing FromRequest can be a handler argument
//! struct MyExtractor(String);
//!
//! impl FromRequest for MyExtractor {
//!     fn from_request(req: &mut Request) -> Result<Self, ladoo::response::Response> {
//!         Ok(MyExtractor(req.path().to_string()))
//!     }
//! }
//!
//! let mut req = Request::test(Method::GET, "/hello");
//! let extracted = MyExtractor::from_request(&mut req).unwrap();
//! assert_eq!(extracted.0, "/hello");
//! ```

use crate::request::Request;
use crate::response::Response;

/// Extract a value from an HTTP request.
///
/// Implement this trait to create custom handler argument types.
/// The framework calls `from_request` for each handler argument
/// before invoking the handler. If extraction fails, the error
/// response is sent to the client and the handler is not called.
///
/// # Examples
///
/// ```
/// use ladoo::extract::FromRequest;
/// use ladoo::request::Request;
/// use ladoo::response::Response;
///
/// struct PathString(String);
///
/// impl FromRequest for PathString {
///     fn from_request(req: &mut Request) -> Result<Self, Response> {
///         Ok(PathString(req.path().to_string()))
///     }
/// }
/// ```
pub trait FromRequest: Sized {
    /// Extract this type from the request.
    ///
    /// Returns `Ok(Self)` on success, or `Err(Response)` with an error
    /// response to send to the client.
    #[allow(clippy::result_large_err)]
    fn from_request(req: &mut Request) -> Result<Self, Response>;
}

#[cfg(feature = "json")]
mod query;

#[cfg(feature = "json")]
pub use query::Query;

#[cfg(feature = "json")]
mod json;

#[cfg(feature = "json")]
pub use json::Json;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::IntoResponse;
    use http::{Method, StatusCode};

    struct PathExtractor(String);

    impl FromRequest for PathExtractor {
        fn from_request(req: &mut Request) -> Result<Self, Response> {
            Ok(PathExtractor(req.path().to_string()))
        }
    }

    #[test]
    fn custom_extractor_extracts_path() {
        let mut req = Request::test(Method::GET, "/users/42");
        let extracted = PathExtractor::from_request(&mut req).unwrap();
        assert_eq!(extracted.0, "/users/42");
    }

    #[test]
    fn extractor_can_fail() {
        struct MustHaveBody;

        impl FromRequest for MustHaveBody {
            fn from_request(req: &mut Request) -> Result<Self, Response> {
                if req.body().is_empty() {
                    Err((StatusCode::BAD_REQUEST, "Body required").into_response())
                } else {
                    Ok(MustHaveBody)
                }
            }
        }

        let mut req = Request::test(Method::GET, "/");
        let result = MustHaveBody::from_request(&mut req);
        assert!(result.is_err());
    }

    #[test]
    fn multiple_extractors_from_same_request() {
        struct MethodExtractor(String);
        impl FromRequest for MethodExtractor {
            fn from_request(req: &mut Request) -> Result<Self, Response> {
                Ok(MethodExtractor(req.method().to_string()))
            }
        }

        let mut req = Request::test(Method::POST, "/submit");
        let path = PathExtractor::from_request(&mut req).unwrap();
        let method = MethodExtractor::from_request(&mut req).unwrap();
        assert_eq!(path.0, "/submit");
        assert_eq!(method.0, "POST");
    }
}
