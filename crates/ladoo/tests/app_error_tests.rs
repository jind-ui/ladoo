#![cfg(feature = "macros")]

use ladoo::prelude::*;

#[derive(Debug, AppError)]
enum UserError {
    #[error(status = 404, message = "user not found")]
    NotFound,
    #[error(status = 409, message = "email already taken")]
    DuplicateEmail(String),
    #[error(status = 422)]
    InvalidAge,
}

#[test]
fn app_error_display_uses_message() {
    let err = UserError::NotFound;
    assert_eq!(format!("{err}"), "user not found");
}

#[test]
fn app_error_display_defaults_to_variant_name() {
    let err = UserError::InvalidAge;
    assert_eq!(format!("{err}"), "InvalidAge");
}

#[test]
fn app_error_into_response_sets_status() {
    std::env::set_var("LADOO_ENV", "production");
    let resp = UserError::NotFound.into_response();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_into_response_conflict() {
    std::env::set_var("LADOO_ENV", "production");
    let resp = UserError::DuplicateEmail("test@example.com".into()).into_response();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_default_status_is_500() {
    std::env::set_var("LADOO_ENV", "production");
    #[derive(Debug, AppError)]
    enum OtherError {
        Unknown,
    }
    let resp = OtherError::Unknown.into_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_struct_variant() {
    #[derive(Debug, AppError)]
    enum ValidationError {
        #[error(status = 422, message = "invalid field")]
        InvalidField { name: String, reason: String },
    }
    std::env::set_var("LADOO_ENV", "production");
    let resp = ValidationError::InvalidField {
        name: "age".into(),
        reason: "must be positive".into(),
    }
    .into_response();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_unit_variant() {
    #[derive(Debug, AppError)]
    enum SimpleError {
        #[error(status = 503, message = "service unavailable")]
        Unavailable,
    }
    let resp = SimpleError::Unavailable.into_response();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn app_error_message_only() {
    #[derive(Debug, AppError)]
    enum MsgError {
        #[error(message = "custom message")]
        Custom,
    }
    std::env::set_var("LADOO_ENV", "production");
    let resp = MsgError::Custom.into_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_no_attribute() {
    #[derive(Debug, AppError)]
    enum BareError {
        Something,
    }
    assert_eq!(format!("{}", BareError::Something), "Something");
    std::env::set_var("LADOO_ENV", "production");
    let resp = BareError::Something.into_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    std::env::remove_var("LADOO_ENV");
}

#[test]
fn app_error_result_as_handler_return() {
    let result: std::result::Result<&str, UserError> = Err(UserError::NotFound);
    let resp = result.into_response();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[test]
fn app_error_ok_result_renders_value() {
    let result: std::result::Result<&str, UserError> = Ok("success");
    let resp = result.into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.body_bytes(), b"success");
}

#[test]
fn app_error_tuple_variant() {
    #[derive(Debug, AppError)]
    enum TupleError {
        #[error(status = 400, message = "bad request")]
        Bad(String, u32),
    }
    let resp = TupleError::Bad("field".into(), 42).into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
