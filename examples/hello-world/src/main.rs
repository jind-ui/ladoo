use ladoo::prelude::*;

fn main() {
    App::new()
        .get("/", |_: Request| "Hello World")
        .get("/users/:id", |req: Request| {
            let id = req.param("id").unwrap_or("0");
            format!("User {id}")
        })
        .run("0.0.0.0:3000");
}
