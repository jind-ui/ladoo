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

#[cfg(feature = "validation")]
impl ValidationErrors {
    /// Convert from the `validator` crate's error type.
    ///
    /// Walks the error tree, extracting `message` from each
    /// [`validator::ValidationError`] when present, falling back to the
    /// error `code`. Nested struct errors are flattened with dot notation
    /// (e.g., `"address.city"`), and nested collection errors (from a
    /// `Vec<T>` field) are flattened with an index suffix
    /// (e.g., `"items[0].name"`).
    pub fn from_validator_errors(errors: validator::ValidationErrors) -> Self {
        let mut result = Self::new();
        Self::flatten_validator_errors("", &errors, &mut result);
        result
    }

    fn flatten_validator_errors(
        prefix: &str,
        errors: &validator::ValidationErrors,
        result: &mut ValidationErrors,
    ) {
        for (field, kind) in errors.errors() {
            let full_field = if prefix.is_empty() {
                field.to_string()
            } else {
                format!("{prefix}.{field}")
            };

            match kind {
                validator::ValidationErrorsKind::Field(field_errors) => {
                    for error in field_errors {
                        let message = error
                            .message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| {
                                format!("validation failed: {}", error.code)
                            });
                        result.add(full_field.clone(), message);
                    }
                }
                validator::ValidationErrorsKind::Struct(nested) => {
                    Self::flatten_validator_errors(&full_field, nested, result);
                }
                validator::ValidationErrorsKind::List(map) => {
                    for (index, nested) in map {
                        let indexed = format!("{full_field}[{index}]");
                        Self::flatten_validator_errors(&indexed, nested, result);
                    }
                }
            }
        }
    }
}

#[cfg(feature = "validation")]
impl<T: validator::Validate> Validate for T {
    fn validate(&self) -> Result<(), ValidationErrors> {
        validator::Validate::validate(self)
            .map_err(ValidationErrors::from_validator_errors)
    }
}

/// Validation extractor — wraps any inner extractor, running validation
/// after extraction.
///
/// `Valid<T>` requires `T: FromRequest + Deref` where `T::Target: Validate`.
/// It first extracts `T` from the request (delegating to `T::from_request`),
/// then calls `validate()` on the inner value. If validation fails, a 422
/// response with per-field errors is returned.
///
/// # Examples
///
/// This example requires the `json` feature (on by default) for `Json`.
#[cfg_attr(feature = "json", doc = "```")]
#[cfg_attr(not(feature = "json"), doc = "```ignore")]
/// use ladoo::extract::{Valid, Validate, ValidationErrors, Json, FromRequest};
/// use ladoo::request::Request;
/// use http::Method;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize)]
/// struct CreateUser { name: String }
///
/// impl Validate for CreateUser {
///     fn validate(&self) -> Result<(), ValidationErrors> {
///         let mut errors = ValidationErrors::new();
///         if self.name.is_empty() {
///             errors.add("name", "must not be empty");
///         }
///         if errors.is_empty() { Ok(()) } else { Err(errors) }
///     }
/// }
///
/// // Valid input passes through
/// let body = br#"{"name":"Alice"}"#;
/// let mut req = Request::test_with_json(Method::POST, "/users", body);
/// let Valid(Json(user)) = Valid::<Json<CreateUser>>::from_request(&mut req).unwrap();
/// assert_eq!(user.name, "Alice");
///
/// // Invalid input returns 422
/// let body = br#"{"name":""}"#;
/// let mut req = Request::test_with_json(Method::POST, "/users", body);
/// let err = Valid::<Json<CreateUser>>::from_request(&mut req).unwrap_err();
/// assert_eq!(err.status(), 422);
/// ```
#[derive(Debug)]
pub struct Valid<T>(pub T);

impl<T> std::ops::Deref for Valid<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> super::FromRequest for Valid<T>
where
    T: super::FromRequest + std::ops::Deref,
    T::Target: Validate,
{
    fn from_request(req: &mut crate::request::Request) -> Result<Self, Response> {
        let inner = T::from_request(req)?;
        inner.deref().validate().map_err(|e| e.into_response())?;
        Ok(Valid(inner))
    }
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

    #[cfg(feature = "json")]
    use super::super::FromRequest;
    #[cfg(feature = "json")]
    use crate::request::Request;
    #[cfg(feature = "json")]
    use http::Method;

    #[cfg(feature = "json")]
    #[derive(Debug, serde::Deserialize)]
    struct CreateUser {
        name: String,
        email: String,
    }

    #[cfg(feature = "json")]
    impl Validate for CreateUser {
        fn validate(&self) -> Result<(), ValidationErrors> {
            let mut errors = ValidationErrors::new();
            if self.name.is_empty() {
                errors.add("name", "must not be empty");
            }
            if !self.email.contains('@') {
                errors.add("email", "must be a valid email address");
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            }
        }
    }

    #[cfg(feature = "json")]
    fn json_request(body: &[u8]) -> Request {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Request::new(
            Method::POST,
            "/users".parse().unwrap(),
            headers,
            Vec::new(),
            bytes::Bytes::copy_from_slice(body),
            std::sync::Arc::new(crate::state::TypeMap::new()),
        )
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_json_passes_through() {
        let body = br#"{"name":"Alice","email":"alice@example.com"}"#;
        let mut req = json_request(body);
        let result =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req);
        assert!(result.is_ok());
        let Valid(json) = result.unwrap();
        assert_eq!(json.name, "Alice");
    }

    #[cfg(feature = "json")]
    #[test]
    fn invalid_json_returns_422() {
        let body = br#"{"name":"","email":"not-an-email"}"#;
        let mut req = json_request(body);
        let result =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(feature = "json")]
    #[test]
    fn invalid_json_response_has_field_errors() {
        let body = br#"{"name":"","email":"bad"}"#;
        let mut req = json_request(body);
        let resp =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap_err();
        let json: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert!(json["fields"]["name"].is_array());
        assert!(json["fields"]["email"].is_array());
    }

    #[cfg(feature = "json")]
    #[test]
    fn multiple_field_errors_in_response() {
        let body = br#"{"name":"","email":"bad"}"#;
        let mut req = json_request(body);
        let resp =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap_err();
        let json: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(json["fields"]["name"][0], "must not be empty");
        assert_eq!(json["fields"]["email"][0], "must be a valid email address");
    }

    #[cfg(feature = "json")]
    #[test]
    fn malformed_json_still_returns_400_not_422() {
        let body = b"not valid json at all";
        let mut req = json_request(body);
        let resp =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap_err();
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
    }

    #[cfg(feature = "json")]
    #[test]
    fn wrong_content_type_still_returns_415() {
        let mut req =
            Request::test_with_body(Method::POST, "/users", b"some body");
        let resp =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap_err();
        assert_eq!(resp.status(), http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_deref_accesses_inner() {
        let body = br#"{"name":"Bob","email":"bob@example.com"}"#;
        let mut req = json_request(body);
        let valid =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap();
        assert_eq!(valid.name, "Bob");
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_destructure_pattern() {
        let body = br#"{"name":"Charlie","email":"c@example.com"}"#;
        let mut req = json_request(body);
        let Valid(json) =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req)
                .unwrap();
        assert_eq!(json.name, "Charlie");
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_query_composability() {
        #[derive(Debug, serde::Deserialize)]
        struct Search {
            q: String,
        }
        impl Validate for Search {
            fn validate(&self) -> Result<(), ValidationErrors> {
                let mut errors = ValidationErrors::new();
                if self.q.is_empty() {
                    errors.add("q", "must not be empty");
                }
                if errors.is_empty() { Ok(()) } else { Err(errors) }
            }
        }

        let mut req = Request::test(Method::GET, "/search?q=rust");
        let result =
            Valid::<super::super::Query<Search>>::from_request(&mut req);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().q, "rust");
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_query_invalid_returns_422() {
        #[derive(Debug, serde::Deserialize)]
        struct Search {
            q: String,
        }
        impl Validate for Search {
            fn validate(&self) -> Result<(), ValidationErrors> {
                let mut errors = ValidationErrors::new();
                if self.q.is_empty() {
                    errors.add("q", "must not be empty");
                }
                if errors.is_empty() { Ok(()) } else { Err(errors) }
            }
        }

        let mut req = Request::test(Method::GET, "/search?q=");
        let resp =
            Valid::<super::super::Query<Search>>::from_request(&mut req)
                .unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(feature = "json")]
    #[test]
    fn valid_only_validates_once() {
        let body = br#"{"name":"Alice","email":"a@b.com"}"#;
        let mut req = json_request(body);
        let result =
            Valid::<super::super::Json<CreateUser>>::from_request(&mut req);
        assert!(result.is_ok());
    }

    #[cfg(feature = "json")]
    #[test]
    fn validation_response_always_json_in_prod() {
        let _guard = crate::error::tests::lock_env();
        std::env::set_var("LADOO_ENV", "production");
        let mut errors = ValidationErrors::new();
        errors.add("field", "error");
        let resp = errors.into_response();
        std::env::remove_var("LADOO_ENV");
        assert_eq!(resp.content_type(), Some("application/json"));
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(feature = "validation")]
    mod validator_integration {
        // Deliberately narrower than `use super::*;`: this module must NOT
        // bring Ladoo's own `Validate` trait into scope. The `validator`
        // crate's `#[validate(nested)]` expansion calls a nested field's
        // `.validate()` unqualified, resolving against whatever `Validate`
        // trait is in scope; if both Ladoo's blanket-impl `Validate` and
        // `validator::Validate` were in scope here, that call would be
        // ambiguous for any type covered by the blanket impl.
        use super::{Valid, ValidationErrors};
        use super::super::super::FromRequest;
        use http::StatusCode;

        #[test]
        fn from_validator_errors_converts_simple_fields() {
            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Input {
                #[validate(length(min = 1, message = "must not be empty"))]
                name: String,
                #[validate(email(message = "must be a valid email"))]
                email: String,
            }

            let input = Input {
                name: String::new(),
                email: "not-email".to_string(),
            };
            let result = validator::Validate::validate(&input);
            assert!(result.is_err());
            let ve = ValidationErrors::from_validator_errors(result.unwrap_err());
            assert!(ve.field_errors().contains_key("name"));
            assert!(ve.field_errors().contains_key("email"));
        }

        #[test]
        fn from_validator_errors_uses_message_when_present() {
            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Input {
                #[validate(length(min = 1, message = "name is required"))]
                name: String,
            }

            let input = Input { name: String::new() };
            let result = validator::Validate::validate(&input);
            let ve = ValidationErrors::from_validator_errors(result.unwrap_err());
            assert!(
                ve.field_errors()["name"]
                    .iter()
                    .any(|m| m.contains("name is required")),
            );
        }

        #[test]
        fn from_validator_errors_falls_back_to_code() {
            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Input {
                #[validate(email)]
                email: String,
            }

            let input = Input { email: "bad".to_string() };
            let result = validator::Validate::validate(&input);
            let ve = ValidationErrors::from_validator_errors(result.unwrap_err());
            let msgs = &ve.field_errors()["email"];
            assert!(!msgs.is_empty());
        }

        #[test]
        fn blanket_impl_works_with_valid_extractor() {
            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct CreateUser {
                #[validate(length(min = 1))]
                name: String,
                #[validate(email)]
                email: String,
            }

            let body = br#"{"name":"Alice","email":"alice@example.com"}"#;
            let mut req = super::json_request(body);
            let result = Valid::<super::super::super::Json<CreateUser>>::from_request(
                &mut req,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn blanket_impl_422_on_invalid() {
            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct CreateUser {
                #[validate(length(min = 1))]
                name: String,
                #[validate(email)]
                email: String,
            }

            let body = br#"{"name":"","email":"bad"}"#;
            let mut req = super::json_request(body);
            let result = Valid::<super::super::super::Json<CreateUser>>::from_request(
                &mut req,
            );
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().status(),
                StatusCode::UNPROCESSABLE_ENTITY,
            );
        }

        #[test]
        fn nested_struct_validation_dot_notation() {
            // The `#[validate(nested)]` expansion calls the nested field's
            // `.validate()` unqualified, so `validator::Validate` must be
            // the only `Validate` trait in scope here (see module-level
            // comment on the import list above).
            use validator::Validate;

            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Address {
                #[validate(length(min = 1))]
                city: String,
            }

            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct User {
                #[validate(length(min = 1))]
                name: String,
                #[validate(nested)]
                address: Address,
            }

            let input = User {
                name: String::new(),
                address: Address { city: String::new() },
            };
            let result = validator::Validate::validate(&input);
            let ve = ValidationErrors::from_validator_errors(result.unwrap_err());
            assert!(
                ve.field_errors().contains_key("address.city")
                    || ve.field_errors().contains_key("city"),
                "nested field should be present: {:?}",
                ve.field_errors(),
            );
        }

        #[test]
        fn nested_list_validation_index_notation() {
            // Same scoping requirement as `nested_struct_validation_dot_notation`:
            // `#[validate(nested)]` on a `Vec<T>` field validates each item by
            // calling its `.validate()` unqualified, which triggers the
            // `ValidationErrorsKind::List` branch in `flatten_validator_errors`.
            use validator::Validate;

            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Item {
                #[validate(length(min = 1))]
                name: String,
            }

            #[derive(Debug, serde::Deserialize, validator::Validate)]
            struct Order {
                #[validate(nested)]
                items: Vec<Item>,
            }

            let input = Order {
                items: vec![
                    Item { name: "widget".to_string() },
                    Item { name: String::new() },
                ],
            };
            let result = validator::Validate::validate(&input);
            assert!(result.is_err());
            let ve = ValidationErrors::from_validator_errors(result.unwrap_err());
            assert!(
                ve.field_errors().contains_key("items[1].name"),
                "indexed list field should be present: {:?}",
                ve.field_errors(),
            );
            assert!(!ve.field_errors().contains_key("items[0].name"));
        }
    }
}
