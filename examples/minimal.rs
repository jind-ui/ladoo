//! Minimal Ladoo application using only the always-on core.
//!
//! Compile with: `cargo check --example minimal --no-default-features`

use ladoo::prelude::*;

fn main() {
    App::new()
        .get("/", |_: Request| "Hello from minimal Ladoo!")
        .get("/health", |_: Request| "OK")
        .run("0.0.0.0:3000");
}
