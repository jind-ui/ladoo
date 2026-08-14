//! Integration tests for `Path<T>` extractor exercised through the public
//! `App` and `TestClient` API — single values, tuples, structs, and error
//! cases, the way a real user would wire them up.

#![cfg(feature = "json")]

use ladoo::prelude::*;
use serde::Deserialize;

#[tokio::test]
async fn single_param_u64() {
    let client = App::test()
        .get("/users/:id", |mut req: Request| {
            let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
            format!("user:{id}")
        })
        .into_client();

    let resp = client.get("/users/42").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "user:42");
}

#[tokio::test]
async fn single_param_string() {
    let client = App::test()
        .get("/users/:name", |mut req: Request| {
            let Path(name) = Path::<String>::from_request(&mut req).unwrap();
            format!("hello:{name}")
        })
        .into_client();

    let resp = client.get("/users/alice").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "hello:alice");
}

#[tokio::test]
async fn tuple_params() {
    let client = App::test()
        .get("/orgs/:org/repos/:id", |mut req: Request| {
            let Path((org, id)) = Path::<(String, u64)>::from_request(&mut req).unwrap();
            format!("{org}/{id}")
        })
        .into_client();

    let resp = client.get("/orgs/acme/repos/7").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "acme/7");
}

#[tokio::test]
async fn struct_params() {
    #[derive(Deserialize)]
    struct ItemParams {
        category: String,
        id: u64,
    }

    let client = App::test()
        .get("/items/:category/:id", |mut req: Request| {
            let Path(p) = Path::<ItemParams>::from_request(&mut req).unwrap();
            format!("{}:{}", p.category, p.id)
        })
        .into_client();

    let resp = client.get("/items/books/99").send().await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "books:99");
}

#[tokio::test]
async fn type_mismatch_returns_400() {
    let client = App::test()
        .get("/users/:id", |mut req: Request| {
            let Path(id) = Path::<u64>::from_request(&mut req)?;
            Ok::<_, Response>(format!("user:{id}"))
        })
        .into_client();

    let resp = client.get("/users/abc").send().await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn path_with_json_body() {
    #[derive(Deserialize)]
    struct Update {
        name: String,
    }

    let client = App::test()
        .put("/users/:id", |mut req: Request| {
            let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
            let Json(body) = Json::<Update>::from_request(&mut req).unwrap();
            format!("update user {id}: {}", body.name)
        })
        .into_client();

    let resp = client
        .put("/users/42")
        .header("Content-Type", "application/json")
        .body(br#"{"name":"Bob"}"#)
        .send()
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "update user 42: Bob");
}

#[tokio::test]
async fn path_does_not_consume_body() {
    let client = App::test()
        .post("/echo/:id", |mut req: Request| {
            let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
            let body = String::from_utf8_lossy(req.body()).to_string();
            format!("{id}:{body}")
        })
        .into_client();

    let resp = client
        .post("/echo/5")
        .body(b"hello")
        .send()
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text(), "5:hello");
}
