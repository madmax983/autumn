use autumn_web::prelude::*;
use autumn_web::static_gen::StaticParams;

async fn list_slugs(_router: autumn_web::reexports::axum::Router) -> Vec<StaticParams> {
    vec![]
}

#[static_get("/about", params = list_slugs)]
async fn about() -> &'static str {
    "about"
}

fn main() {}
