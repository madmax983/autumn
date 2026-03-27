use autumn_web::prelude::*;

#[static_get("/about")]
async fn about() -> &'static str {
    "About page"
}

fn main() {
    // Verify both companion functions exist
    let _route = __autumn_route_info_about();
    let _meta = __autumn_static_meta_about();
}
