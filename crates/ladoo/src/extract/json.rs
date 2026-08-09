//! JSON request extractor and response type.
//!
//! `Json<T>` works both ways:
//! - As a handler argument: deserializes the request body from JSON
//! - As a return type: serializes `T` to JSON and sets `Content-Type: application/json`
//!
//! # Examples
//!
//! ```
//! use ladoo::extract::{Json, FromRequest};
//! use ladoo::request::Request;
//! use ladoo::response::IntoResponse;
//! use http::Method;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize)]
//! struct CreateUser {
//!     name: String,
//! }
//!
//! #[derive(Serialize)]
//! struct User {
//!     id: u64,
//!     name: String,
//! }
//!
//! // As a response — serializes to JSON with the right Content-Type.
//! let resp = Json(User { id: 1, name: "Alice".to_string() }).into_response();
//! assert_eq!(resp.content_type(), Some("application/json"));
//! ```

use bytes::Bytes;
use http::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::ops::Deref;

use super::FromRequest;
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// Extract JSON from the request body, or serialize a value as a JSON response.
///
/// # As extractor
///
/// Deserializes the request body as `T`. Requires `Content-Type: application/json`.
/// Returns 415 if the content type is missing or wrong, or 400 if the body
/// is not valid JSON for `T`.
///
/// # As response
///
/// Serializes `T` to JSON with `Content-Type: application/json`.
///
/// # Examples
///
/// ```
/// use ladoo::extract::Json;
/// use ladoo::response::IntoResponse;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Greeting {
///     message: String,
/// }
///
/// let resp = Json(Greeting { message: "hi".to_string() }).into_response();
/// assert_eq!(resp.status(), 200);
/// ```
#[derive(Debug)]
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned,
{
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        let content_type = req.content_type().unwrap_or("");
        if !content_type.starts_with("application/json") {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Expected Content-Type: application/json",
            )
                .into_response());
        }
        let body = req.take_body();
        serde_json::from_slice(&body)
            .map(Json)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {e}")).into_response())
    }
}

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => {
                let mut headers = http::HeaderMap::new();
                headers.insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("application/json"),
                );
                Response::new(StatusCode::OK, headers, Bytes::from(bytes))
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("JSON serialization error: {e}"),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct CreateUser {
        name: String,
        email: String,
    }

    fn json_request(method: Method, path: &str, body: &[u8]) -> Request {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Request::new(
            method,
            path.parse().unwrap(),
            headers,
            Vec::new(),
            Bytes::copy_from_slice(body),
            std::sync::Arc::new(crate::state::TypeMap::new()),
        )
    }

    #[test]
    fn extracts_json_body() {
        let body = br#"{"name":"Alice","email":"alice@example.com"}"#;
        let mut req = json_request(Method::POST, "/users", body);
        let json = Json::<CreateUser>::from_request(&mut req).unwrap();
        assert_eq!(json.name, "Alice");
        assert_eq!(json.email, "alice@example.com");
    }

    #[test]
    fn wrong_content_type_returns_415() {
        let mut req = Request::test_with_body(Method::POST, "/users", b"not json");
        let result = Json::<CreateUser>::from_request(&mut req);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[test]
    fn invalid_json_returns_400() {
        let mut req = json_request(Method::POST, "/users", b"not valid json");
        let result = Json::<CreateUser>::from_request(&mut req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn takes_body_so_second_read_is_empty() {
        let body = br#"{"name":"Bob","email":"bob@example.com"}"#;
        let mut req = json_request(Method::POST, "/users", body);
        let _json = Json::<CreateUser>::from_request(&mut req).unwrap();
        assert!(req.body().is_empty());
    }

    #[test]
    fn deref_accesses_inner_value() {
        let body = br#"{"name":"Charlie","email":"c@example.com"}"#;
        let mut req = json_request(Method::POST, "/users", body);
        let json = Json::<CreateUser>::from_request(&mut req).unwrap();
        assert_eq!(json.name, "Charlie");
    }

    #[derive(Serialize)]
    struct UserResponse {
        id: u64,
        name: String,
    }

    #[test]
    fn json_response_serializes_to_json() {
        let resp = Json(UserResponse {
            id: 1,
            name: "Alice".to_string(),
        })
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.content_type(), Some("application/json"));
        let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["id"], 1);
        assert_eq!(body["name"], "Alice");
    }

    #[test]
    fn json_response_with_status_code() {
        let resp = (
            StatusCode::CREATED,
            Json(UserResponse {
                id: 42,
                name: "Bob".to_string(),
            }),
        )
            .into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(resp.content_type(), Some("application/json"));
    }
}
