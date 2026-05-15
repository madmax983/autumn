mod controllers;
mod models;

use axum::{routing, Router};
use controllers::posts;
use loco_rs::{
    app::{AppContext, Hooks},
    bgworker::Queue,
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
    Result,
};
use sea_orm_migration::prelude::*;
use std::path::Path;

#[path = "../migrations/m20240101_000001_create_posts.rs"]
mod m20240101_000001_create_posts;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20240101_000001_create_posts::Migration)]
    }
}

pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/api/posts", routing::get(posts::list).post(posts::create))
        .route("/api/posts/protected", routing::get(posts::protected))
        .route(
            "/api/posts/{id}",
            routing::get(posts::show)
                .patch(posts::update)
                .delete(posts::delete),
        )
        .route("/posts", routing::get(posts::html_list))
        .route("/posts/{id}", routing::get(posts::html_show))
}

#[tokio::main]
async fn main() -> Result<()> {
    loco_rs::cli::main::<BenchApp, Migrator>().await
}

struct BenchApp;

#[async_trait::async_trait]
impl Hooks for BenchApp {
    fn app_name() -> &'static str {
        "bench-loco"
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(
            loco_rs::controller::Routes::new()
                .prefix("")
                .add("/api/posts", routing::get(posts::list).post(posts::create))
                .add("/api/posts/protected", routing::get(posts::protected))
                .add(
                    "/api/posts/{id}",
                    routing::get(posts::show)
                        .patch(posts::update)
                        .delete(posts::delete),
                )
                .add("/posts", routing::get(posts::html_list))
                .add("/posts/{id}", routing::get(posts::html_show)),
        )
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    fn register_tasks(_tasks: &mut Tasks) {}

    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }

    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        Ok(())
    }
}
