use autumn_web::prelude::*;

#[static_get("/posts/{slug}")]
async fn show() -> &'static str {
    "post"
}

fn main() {}
