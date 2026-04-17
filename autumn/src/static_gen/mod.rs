//! Static Site Generation (SSG) and Incremental Static Regeneration (ISR).
//!
//! The `static_gen` module bridges the gap between dynamic Axum handlers and statically built HTML.
//! By annotating routes with [`#[static_get]`](crate::static_get), you can pre-render responses at build time, significantly
//! reducing server load and improving client-side latency. This module powers the `autumn build` CLI command.
//!
//! # High-Level Concept
//!
//! Autumn allows you to mix standard dynamic routes (e.g. `#[get]`) with static routes (e.g. `#[static_get]`).
//! During a build step, [`crate::static_gen::render_static_routes`] invokes the static handlers, capturing the returned HTML and
//! writing it to a `dist/` directory alongside a [`crate::static_gen::StaticManifest`].
//!
//! In production, the [`crate::static_gen::StaticFileLayer`] middleware intercepts requests matching these routes. If a generated
//! HTML file is available, it serves it instantly. If the route specifies an ISR `revalidate` interval, the
//! middleware will transparently refresh stale content in the background without blocking the user.
//!
//! # Usage
//!
//! This module is primarily meant to be used via the `#[static_get]` macro.
//!
//! ## Simple Pre-rendered Route
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! /// A route that renders once at build time.
//! #[static_get("/about")]
//! async fn about() -> &'static str {
//!     "About us"
//! }
//! ```
//!
//! ## Parameterized Routes with ISR
//!
//! If your route has parameters, you must provide a `params_fn` to tell the build engine which parameters to pre-render.
//! You can also provide a `revalidate` interval (in seconds) to enable Incremental Static Regeneration.
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::static_gen::StaticParams;
//! use std::future::Future;
//! use std::pin::Pin;
//!
//! /// The function that provides parameters for the build step.
//! fn blog_params(_router: axum::Router) -> Pin<Box<dyn Future<Output = Vec<StaticParams>> + Send>> {
//!     Box::pin(async {
//!         vec![
//!             autumn_web::static_params! { "slug" => "hello-world" },
//!             autumn_web::static_params! { "slug" => "rust-is-awesome" },
//!         ]
//!     })
//! }
//!
//! /// A route that pre-renders for specific parameters, and refreshes every hour.
//! #[static_get(path = "/posts/{slug}", revalidate = 3600, params_fn = blog_params)]
//! async fn show_post(Path(slug): Path<String>) -> String {
//!     format!("Post: {slug}")
//! }
//! ```
//!
//! # Architecture
//!
//! The SSG pipeline has two main components:
//!
//! 1. **Build Time ([`crate::static_gen::build`]):** The `autumn build` command constructs a router, mocks incoming requests, and records
//!    the `200 OK` responses to disk, generating a `manifest.json`.
//! 2. **Runtime ([`middleware`]):** When the application starts in production, [`crate::static_gen::StaticFileLayer`] loads the manifest.
//!    Requests for static routes bypass the dynamic handler and read the HTML from disk.
//!
//! # Panics
//!
//! The static generation engine does not panic on route handler errors; instead, it returns a [`crate::static_gen::BuildError`]
//! and halts the build. You must ensure your handlers always return a successful `2xx` HTTP response during the build step.
//!
//! # Performance
//!
//! The [`crate::static_gen::render_static_routes`] function renders routes concurrently (default limit of 8 simultaneous requests) to speed up
//! massive SSG builds. The [`crate::static_gen::StaticFileLayer`] middleware serves from the filesystem and caches the manifest in memory,
//! providing zero-overhead routing for static assets.

pub mod build;
mod middleware;
mod types;

pub use build::{BuildError, render_static_routes};
pub use middleware::StaticFileLayer;
pub use types::{
    ManifestEntry, ParamsFn, StaticManifest, StaticParams, StaticRouteMeta, url_to_file_path,
};
