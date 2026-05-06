//! Static Site Generation (SSG) and Incremental Static Regeneration (ISR).
//!
//! Sometimes, rendering a page on every request is just too slow. For pages where the
//! data changes rarely (like a blog post or a marketing landing page), it is much faster
//! to generate the HTML once at build time.
//!
//! This module provides the engine to pre-render routes annotated with `#[static_get]`
//! and serve them lightning-fast via the [`crate::static_gen::StaticFileLayer`].
//!
//! ## How it works
//!
//! 1. **Build phase:** `autumn build` discovers your `#[static_get]` routes and runs them, saving the HTML to `dist/`.
//! 2. **Runtime phase:** The `StaticFileLayer` intercepts requests. If a pre-rendered file exists, it serves it directly!

pub(crate) mod build;
pub mod isr_coordinator;
mod middleware;
mod types;

pub use build::{BuildError, render_static_routes};
pub use isr_coordinator::{IsrCoordinator, LocalIsrCoordinator, isr_advisory_lock_key, isr_window_key};
#[cfg(feature = "db")]
pub use isr_coordinator::PostgresIsrCoordinator;
pub use middleware::StaticFileLayer;
pub use types::{
    ManifestEntry, ParamsFn, StaticManifest, StaticParams, StaticRouteMeta, url_to_file_path,
};
