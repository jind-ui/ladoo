//! Query string extractor.
//!
//! Deserializes the URL query string into a typed struct using serde.
//!
//! # Examples
//!
//! ```
//! use ladoo::extract::Query;
//! use ladoo::extract::FromRequest;
//! use ladoo::request::Request;
//! use http::Method;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Params {
//!     page: u32,
//! }
//!
//! let mut req = Request::test(Method::GET, "/users?page=3");
//! let query = Query::<Params>::from_request(&mut req).unwrap();
//! assert_eq!(query.page, 3);
//! ```

use serde::de::DeserializeOwned;
use std::ops::Deref;

use super::FromRequest;
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// Extract typed data from the URL query string.
///
/// The inner type `T` must implement [`serde::Deserialize`].
/// Returns 400 Bad Request if the query string cannot be parsed.
///
/// # Examples
///
/// ```
/// use ladoo::extract::{Query, FromRequest};
/// use ladoo::request::Request;
/// use http::Method;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Search {
///     q: String,
///     page: Option<u32>,
/// }
///
/// let mut req = Request::test(Method::GET, "/search?q=rust&page=2");
/// let search = Query::<Search>::from_request(&mut req).unwrap();
/// assert_eq!(search.q, "rust");
/// assert_eq!(search.page, Some(2));
/// ```
#[derive(Debug)]
pub struct Query<T>(pub T);

impl<T> Deref for Query<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned,
{
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        let query_string = req.uri().query().unwrap_or("");
        serde_urlencoded::from_str(query_string)
            .map(Query)
            .map_err(|e| {
                (
                    http::StatusCode::BAD_REQUEST,
                    format!("Invalid query string: {e}"),
                )
                    .into_response()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    struct Pagination {
        page: u32,
        per_page: Option<u32>,
    }

    #[test]
    fn extracts_query_params() {
        let mut req = Request::test(Method::GET, "/users?page=2&per_page=10");
        let query = Query::<Pagination>::from_request(&mut req).unwrap();
        assert_eq!(query.page, 2);
        assert_eq!(query.per_page, Some(10));
    }

    #[test]
    fn optional_params_default_to_none() {
        let mut req = Request::test(Method::GET, "/users?page=1");
        let query = Query::<Pagination>::from_request(&mut req).unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.per_page, None);
    }

    #[test]
    fn invalid_query_returns_400() {
        let mut req = Request::test(Method::GET, "/users?page=abc");
        let result = Query::<Pagination>::from_request(&mut req);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn empty_query_with_defaults() {
        #[derive(Deserialize, Debug)]
        struct OptionalParams {
            #[serde(default)]
            q: String,
        }

        let mut req = Request::test(Method::GET, "/search");
        let query = Query::<OptionalParams>::from_request(&mut req).unwrap();
        assert_eq!(query.q, "");
    }

    #[test]
    fn deref_accesses_inner() {
        let mut req = Request::test(Method::GET, "/users?page=5");
        let query = Query::<Pagination>::from_request(&mut req).unwrap();
        assert_eq!(query.page, 5);
    }

    #[test]
    fn query_does_not_consume_body() {
        let mut req = Request::test_with_body(Method::POST, "/users?page=1", b"body data");
        let _query = Query::<Pagination>::from_request(&mut req).unwrap();
        assert_eq!(req.body(), b"body data");
    }
}
