use autumn::extract::Path;
use autumn::{delete, get, post, put};

#[get("/hello")]
async fn hello() -> &'static str {
    "hello"
}

#[post("/items")]
async fn create() -> &'static str {
    "created"
}

#[put("/items/{id}")]
async fn update(_id: Path<i32>) -> &'static str {
    "updated"
}

#[delete("/items/{id}")]
async fn remove(_id: Path<i32>) -> &'static str {
    "removed"
}

#[get("/no-return")]
async fn no_return() {}

fn main() {}
