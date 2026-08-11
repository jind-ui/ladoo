//! Validation extractor and error types.
//!
//! `Valid<T>` wraps any extractor that derefs to a `Validate` type,
//! running validation after extraction. Returns 422 with per-field
//! errors on failure.
//!
//! # Examples
//!
//! ```
//! use ladoo::extract::{Validate, ValidationErrors};
//!
//! struct MyInput { name: String }
//!
//! impl Validate for MyInput {
//!     fn validate(&self) -> Result<(), ValidationErrors> {
//!         let mut errors = ValidationErrors::new();
//!         if self.name.is_empty() {
//!             errors.add("name", "must not be empty");
//!         }
//!         if errors.is_empty() { Ok(()) } else { Err(errors) }
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::fmt;

use bytes::Bytes;
use http::StatusCode;

use crate::response::{IntoResponse, Response};

/// Per-field validation errors.
///
/// Wraps a `HashMap<String, Vec<String>>` mapping field names to lists
/// of error messages. Use [`add`](Self::add) to accumulate errors and
/// [`is_empty`](Self::is_empty) to check whether any exist.
///
/// Implements [`IntoResponse`] to render a 422 response: a JSON body with
/// the `fields` map when the `json` feature is enabled (the default), or
/// a plain-text rendering of [`Display`](fmt::Display) otherwise.
#[derive(Debug, Clone)]
pub struct ValidationErrors(HashMap<String, Vec<String>>);

impl ValidationErrors {
    /// Create an empty error set.
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Append an error message to a field.
    pub fn add(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0
            .entry(field.into())
            .or_default()
            .push(message.into());
    }

    /// Returns `true` if no errors have been added.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrow the inner field-to-messages map.
    pub fn field_errors(&self) -> &HashMap<String, Vec<String>> {
        &self.0
    }

    /// Consume and return the inner map.
    pub fn into_inner(self) -> HashMap<String, Vec<String>> {
        self.0
    }
}

impl Default for ValidationErrors {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Validation failed")?;
        let mut fields: Vec<_> = self.0.iter().collect();
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (field, messages) in &fields {
            write!(f, ": {}: {}", field, messages.join(", "))?;
        }
        Ok(())
    }
}

#[cfg(feature = "json")]
impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        let fields: serde_json::Value = self
            .0
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::Value::Array(
                        v.iter().map(|m| serde_json::Value::String(m.clone())).collect(),
                    ),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into();

        let body = serde_json::json!({
            "error": "Validation failed",
            "status": 422,
            "fields": fields,
        });

        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Response::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            headers,
            Bytes::from(body.to_string()),
        )
    }
}

#[cfg(not(feature = "json"))]
impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        Response::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            headers,
            Bytes::from(self.to_string()),
        )
    }
}

/// Validate a value, returning per-field errors on failure.
///
/// Implement this trait on types that should be validated after
/// extraction. For manual validation, accumulate errors with
/// [`ValidationErrors::add`] and return them.
///
/// When the `validation` feature is enabled, a blanket impl covers
/// all types that implement [`validator::Validate`], so
/// `#[derive(validator::Validate)]` types work automatically.
///
/// # Examples
///
/// ```
/// use ladoo::extract::{Validate, ValidationErrors};
///
/// struct Age(u32);
///
/// impl Validate for Age {
///     fn validate(&self) -> Result<(), ValidationErrors> {
///         let mut errors = ValidationErrors::new();
///         if self.0 > 150 {
///             errors.add("age", "must be at most 150");
///         }
///         if errors.is_empty() { Ok(()) } else { Err(errors) }
///     }
/// }
///
/// assert!(Age(25).validate().is_ok());
/// assert!(Age(200).validate().is_err());
/// ```
pub trait Validate {
    /// Validate this value, returning field errors on failure.
    fn validate(&self) -> Result<(), ValidationErrors>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_errors_is_empty() {
        let errors = ValidationErrors::new();
        assert!(errors.is_empty());
    }

    #[test]
    fn add_makes_non_empty() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "is required");
        assert!(!errors.is_empty());
    }

    #[test]
    fn add_multiple_messages_to_same_field() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "is required");
        errors.add("email", "must be valid");
        let field = &errors.field_errors()["email"];
        assert_eq!(field.len(), 2);
        assert_eq!(field[0], "is required");
        assert_eq!(field[1], "must be valid");
    }

    #[test]
    fn add_errors_to_multiple_fields() {
        let mut errors = ValidationErrors::new();
        errors.add("name", "is required");
        errors.add("email", "must be valid");
        assert_eq!(errors.field_errors().len(), 2);
    }

    #[test]
    fn field_errors_borrows_map() {
        let mut errors = ValidationErrors::new();
        errors.add("age", "too young");
        let map = errors.field_errors();
        assert!(map.contains_key("age"));
    }

    #[test]
    fn into_inner_returns_owned_map() {
        let mut errors = ValidationErrors::new();
        errors.add("name", "required");
        let map = errors.into_inner();
        assert_eq!(map["name"], vec!["required".to_string()]);
    }

    #[test]
    fn default_is_empty() {
        let errors = ValidationErrors::default();
        assert!(errors.is_empty());
    }

    #[test]
    fn display_empty() {
        let errors = ValidationErrors::new();
        assert_eq!(format!("{errors}"), "Validation failed");
    }

    #[test]
    fn display_single_field() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "must be a valid email");
        let display = format!("{errors}");
        assert!(display.starts_with("Validation failed"));
        assert!(display.contains("email"));
        assert!(display.contains("must be a valid email"));
    }

    #[test]
    fn display_multiple_fields_sorted() {
        let mut errors = ValidationErrors::new();
        errors.add("name", "required");
        errors.add("age", "must be positive");
        let display = format!("{errors}");
        let age_pos = display.find("age").unwrap();
        let name_pos = display.find("name").unwrap();
        assert!(age_pos < name_pos, "fields should be sorted alphabetically");
    }

    #[test]
    fn into_response_returns_422() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "invalid");
        let resp = errors.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(not(feature = "json"))]
    #[test]
    fn into_response_plain_text_fallback() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "invalid");
        let resp = errors.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(resp.content_type(), Some("text/plain; charset=utf-8"));
        let body = std::str::from_utf8(resp.body_bytes()).unwrap();
        assert!(body.starts_with("Validation failed"));
        assert!(body.contains("email"));
        assert!(body.contains("invalid"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn into_response_is_json() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "invalid");
        let resp = errors.into_response();
        assert_eq!(resp.content_type(), Some("application/json"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn into_response_body_structure() {
        let mut errors = ValidationErrors::new();
        errors.add("email", "must be a valid email address");
        errors.add("age", "must be greater than or equal to 0");
        let resp = errors.into_response();
        let body: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["error"], "Validation failed");
        assert_eq!(body["status"], 422);
        assert_eq!(
            body["fields"]["email"][0],
            "must be a valid email address"
        );
        assert_eq!(
            body["fields"]["age"][0],
            "must be greater than or equal to 0"
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn into_response_empty_errors() {
        let errors = ValidationErrors::new();
        let resp = errors.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["fields"], serde_json::json!({}));
    }

    #[test]
    fn manual_validate_impl_ok() {
        struct MyInput { value: i32 }
        impl Validate for MyInput {
            fn validate(&self) -> Result<(), ValidationErrors> {
                let mut errors = ValidationErrors::new();
                if self.value < 0 {
                    errors.add("value", "must be non-negative");
                }
                if errors.is_empty() { Ok(()) } else { Err(errors) }
            }
        }
        assert!(MyInput { value: 5 }.validate().is_ok());
    }

    #[test]
    fn manual_validate_impl_err() {
        struct MyInput { value: i32 }
        impl Validate for MyInput {
            fn validate(&self) -> Result<(), ValidationErrors> {
                let mut errors = ValidationErrors::new();
                if self.value < 0 {
                    errors.add("value", "must be non-negative");
                }
                if errors.is_empty() { Ok(()) } else { Err(errors) }
            }
        }
        let result = MyInput { value: -1 }.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.field_errors()["value"], vec!["must be non-negative"]);
    }

    #[test]
    fn clone_validation_errors() {
        let mut errors = ValidationErrors::new();
        errors.add("field", "message");
        let cloned = errors.clone();
        assert_eq!(cloned.field_errors()["field"], vec!["message"]);
    }
}
