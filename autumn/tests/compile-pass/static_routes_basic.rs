use autumn_web::prelude::*;

#[static_get("/about")]
async fn about() -> &'static str { "About" }

fn main() {
    let metas: Vec<autumn_web::static_gen::StaticRouteMeta> =
        static_routes![about];
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].path, "/about");
}
