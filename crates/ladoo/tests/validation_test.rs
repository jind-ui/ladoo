//! Integration tests for `Valid<T>` exercised through the public `App` and
//! `TestClient` API, the way a real user would validate a handler input.

#![cfg(feature = "json")]

use ladoo::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct RegisterInput {
    username: String,
    age: u32,
}

impl Validate for RegisterInput {
    fn validate(&self) -> std::result::Result<(), ladoo::extract::ValidationErrors> {
        let mut errors = ladoo::extract::ValidationErrors::new();
        if self.username.len() < 3 {
            errors.add("username", "must be at least 3 characters");
        }
        if self.age > 150 {
            errors.add("age", "must be at most 150");
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

async fn register(Valid(Json(input)): Valid<Json<RegisterInput>>) -> impl IntoResponse {
    format!("Welcome, {}!", input.username)
}

#[tokio::test]
async fn valid_input_passes_through() {
    let client = App::test().post("/register", register).into_client();
    let resp = client
        .post("/register")
        .header("content-type", "application/json")
        .body(br#"{"username":"alice","age":30}"#)
        .send()
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "Welcome, alice!");
}

#[tokio::test]
async fn invalid_input_returns_422_with_field_errors() {
    let client = App::test().post("/register", register).into_client();
    let resp = client
        .post("/register")
        .header("content-type", "application/json")
        .body(br#"{"username":"ab","age":200}"#)
        .send()
        .await;
    assert_eq!(resp.status(), 422);
    let body: serde_json::Value = serde_json::from_slice(resp.body_bytes()).unwrap();
    assert_eq!(body["error"], "Validation failed");
    assert!(body["fields"]["username"].is_array());
    assert!(body["fields"]["age"].is_array());
}

#[tokio::test]
async fn malformed_json_returns_400_not_422() {
    let client = App::test().post("/register", register).into_client();
    let resp = client
        .post("/register")
        .header("content-type", "application/json")
        .body(b"not json")
        .send()
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn wrong_content_type_returns_415() {
    let client = App::test().post("/register", register).into_client();
    let resp = client
        .post("/register")
        .header("content-type", "text/plain")
        .body(br#"{"username":"alice","age":30}"#)
        .send()
        .await;
    assert_eq!(resp.status(), 415);
}
