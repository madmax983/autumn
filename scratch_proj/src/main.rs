use autumn_web::form::skip_link;
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

pub fn layout(title: &str, content: maud::Markup) -> maud::Markup {
    maud::html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/app.css";
            }
            body {
                (skip_link("#main-content", "Skip to main content"))
                header role="banner" {
                    nav aria-label="Main navigation" {
                        a href="/" { "scratch_proj" }
                    }
                }
                main id="main-content" role="main" {
                    (content)
                }
                footer role="contentinfo" {
                    p { "Built with Autumn" }
                }
            }
        }
    }
}

#[get("/")]
async fn index() -> maud::Markup {
    layout("Welcome", maud::html! {
        h1 { "Welcome to scratch_proj!" }
        p { "Edit " code { "src/main.rs" } " to get started." }
    })
}

#[get("/hello")]
async fn hello() -> &'static str {
    "Hello, Autumn!"
}

#[get("/hello/{name}")]
async fn hello_name(name: autumn_web::extract::Path<String>) -> String {
    format!("Hello, {}!", *name)
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, hello, hello_name])
        .migrations(MIGRATIONS)
        .run()
        .await;
}
