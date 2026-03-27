use autumn_web::prelude::*;

#[static_get("/about")]
fn about() -> &'static str {
    "About page"
}

fn main() {}
