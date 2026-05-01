# autumn-web Example Projects

Complete main.rs and Cargo.toml from the official example apps. These show
idiomatic autumn-web patterns at different complexity levels.

## examples/blog — Static pre-rendering, CRUD, admin

```rust
// examples/blog/src/main.rs
mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::{routes, static_routes};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::about::about,       // #[static_get] — pre-rendered
            routes::posts::index,
            routes::posts::show,
            routes::posts::admin_list,
            routes::posts::new_form,
            routes::posts::create,
            routes::posts::edit_form,
            routes::posts::update,
            routes::posts::delete_post,
            routes::api::list_json,
            routes::api::create_json,
        ])
        .static_routes(static_routes![routes::about::about])
        .run()
        .await;
}
```

```toml
# examples/blog/Cargo.toml
[package]
name = "blog"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
autumn-web = { path = "../../autumn" }
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono"] }
diesel-async = { version = "0.8", features = ["postgres"] }
pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }
diesel_migrations = "2"
maud = { version = "0.27", features = ["axum"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
```

## examples/todo-app — Classic CRUD with htmx

```rust
// examples/todo-app/src/main.rs
mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::routes;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::todos::index,
            routes::todos::list,
            routes::todos::detail,
            routes::todos::create,
            routes::todos::toggle,
            routes::todos::delete_todo,
            routes::api::list_json,
            routes::api::create_json,
        ])
        .run()
        .await;
}
```

## examples/reddit-clone — Full-featured (most comprehensive)

This is the reference app for building production-grade autumn-web applications.
It demonstrates every framework feature: auth, sessions, CSRF, `#[secured]`,
`#[model]`, `#[repository]`, mutation hooks, `#[scheduled]`, `#[static_get]`,
`#[ws]`, `#[job]`, plugins, and htmx voting.

```rust
// examples/reddit-clone/src/main.rs
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;
use reddit_clone::{live_events, repositories, routes, tasks};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::posts::front_page,
            routes::about::about,
            routes::auth::register_form,
            routes::auth::register,
            routes::auth::login_form,
            routes::auth::login,
            routes::auth::logout,
            routes::auth::profile,
            routes::subreddits::list,
            routes::subreddits::create_form,
            routes::subreddits::create,
            routes::subreddits::show,
            routes::posts::submit_form,
            routes::posts::submit_to_sub_form,
            routes::posts::submit,
            routes::posts::show,
            routes::posts::edit_form,
            routes::posts::update,
            routes::posts::delete_post,
            routes::comments::create,
            routes::comments::list_comments,
            routes::votes::upvote,
            routes::votes::downvote,
            routes::live::live_feed_health,
            routes::live::live_feed,
            routes::live::subreddit_feed,
            // #[repository]-generated API routes
            repositories::subreddit_api_list,
            repositories::subreddit_api_get,
            repositories::post_api_list,
            repositories::post_api_get,
        ])
        .static_routes(static_routes![routes::about::about])
        .tasks(tasks![
            tasks::recalculate_hot_ranks,
            tasks::prune_live_feed_events,
        ])
        .jobs(reddit_clone::jobs::registered_jobs())
        .plugin(live_events::LiveFeedPlugin::new())
        .run()
        .await;
}
```

```toml
# examples/reddit-clone/Cargo.toml
[package]
name = "reddit-clone"
edition.workspace = true
version.workspace = true
publish = false
default-run = "reddit-clone"


[dependencies]
autumn-web = { path = "../../autumn", features = ["mail", "ws", "storage", "multipart", "redis"] }
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono", "serde_json"] }
diesel-async = { version = "0.8", features = ["postgres"] }
pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }
redis = { version = "1.2", features = ["aio", "tokio-comp"] }
diesel_migrations = "2"
futures = { workspace = true }
maud = { version = "0.27", features = ["axum"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
tracing = "0.1"
validator = { version = "0.20", features = ["derive"] }
scoped-futures = "0.1"
thiserror = { workspace = true }
uuid = { version = "1", features = ["v4"] }
```

### Key patterns from reddit-clone

- **lib.rs**: Exports shared modules so tests and binaries can access them
- **Jobs registration**: `.jobs(reddit_clone::jobs::registered_jobs())` wires typed `#[job]` handlers into the runtime.
- **Plugin registration**: `.plugin(LiveFeedPlugin::new())` starts the durable live-feed relay.
- **Repository-generated API routes**: `repositories::subreddit_api_list` etc. come
  from the `#[repository]` macro, not from hand-written route handlers
- **Static + dynamic routes coexist**: `about` is both in `routes![]` (dynamic) and
  `static_routes![]` (pre-rendered by `autumn build`)
- **Workspace patch**: Root Cargo.toml uses `[patch.crates-io] autumn-web = { path = "autumn" }`
  to unify the local version across all workspace members

