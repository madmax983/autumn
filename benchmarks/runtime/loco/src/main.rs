mod controllers;
mod models;

use axum::{routing, Router};
use loco_rs::app::AppContext;
use controllers::posts;

pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/api/posts",            routing::get(posts::list).post(posts::create))
        .route("/api/posts/protected",  routing::get(posts::protected))
        .route("/api/posts/:id",        routing::get(posts::show).patch(posts::update).delete(posts::delete))
        .route("/posts",                routing::get(posts::html_list))
        .route("/posts/:id",            routing::get(posts::html_show))
}

#[tokio::main]
async fn main() {
    loco_rs::boot::run::<BenchApp>().await.unwrap();
}

struct BenchApp;

#[async_trait::async_trait]
impl loco_rs::app::Hooks for BenchApp {
    fn app_name() -> &'static str { "bench-loco" }

    fn routes(_ctx: &AppContext) -> loco_rs::controller::AppRoutes {
        loco_rs::controller::AppRoutes::with_default_routes()
            .add_route(
                loco_rs::controller::Routes::new()
                    .prefix("")
                    .add("/api/posts",           routing::get(posts::list).post(posts::create))
                    .add("/api/posts/protected", routing::get(posts::protected))
                    .add("/api/posts/:id",       routing::get(posts::show).patch(posts::update).delete(posts::delete))
                    .add("/posts",               routing::get(posts::html_list))
                    .add("/posts/:id",           routing::get(posts::html_show)),
            )
    }
}
