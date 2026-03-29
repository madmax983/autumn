use autumn_web::prelude::*;
use autumn_web::static_gen::StaticParams;
use std::collections::HashMap;

async fn list_slugs(_router: autumn_web::reexports::axum::Router) -> Vec<StaticParams> {
    vec![{
        let mut m = HashMap::new();
        m.insert("slug".to_owned(), "hello".to_owned());
        m
    }]
}

#[static_get("/posts/{slug}", params = list_slugs)]
async fn show_post() -> &'static str {
    "post"
}

#[static_get("/cached", revalidate = 60)]
async fn cached_page() -> &'static str {
    "cached"
}

fn main() {
    // Verify both companion functions exist
    let _route = __autumn_route_info_show_post();
    let _meta = __autumn_static_meta_show_post();

    let _route2 = __autumn_route_info_cached_page();
    let _meta2 = __autumn_static_meta_cached_page();
}
