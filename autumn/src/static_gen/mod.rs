//! Static site generation and serving.
//!
//! This module provides the infrastructure for pre-rendering `#[static_get]` routes
//! into HTML files at build time (`autumn build`), and serving them efficiently at runtime.
//! It also includes support for Incremental Static Regeneration (ISR), which allows
//! specific static routes to be re-rendered in the background after a TTL expires.
//!
//! # Core components
//!
//! * [`build`] - The rendering engine used by the CLI to generate `dist/`.
//! * [`StaticFileLayer`] - The middleware that intercepts requests and serves generated files.
//! * [`StaticRouteMeta`] - The configuration type emitted by the `#[static_get]` macro.

pub mod build;
mod middleware;
mod types;

pub use build::{BuildError, render_static_routes};
pub use middleware::StaticFileLayer;
pub use types::{
    ManifestEntry, ParamsFn, StaticManifest, StaticParams, StaticRouteMeta, url_to_file_path,
};
