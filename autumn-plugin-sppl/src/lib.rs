//! SPPL static asset serving plugin for Autumn applications.
//!
//! Embeds a pre-built static site (`SvelteKit`, Vite, etc.) directly into the
//! binary and serves it from an Autumn application via the plugin system.
//!
//! # Setup
//!
//! Add to `Cargo.toml`:
//! ```toml
//! [dependencies]
//! autumn-plugin-sppl = { git = "https://github.com/madmax983/autumn" }
//!
//! [build-dependencies]
//! sppl = { git = "https://github.com/neutral-engineering/sppl", default-features = false }
//! ```
//!
//! Optionally pre-compress assets at compile time in `build.rs`:
//! ```rust,no_run
//! sppl::build::gzip_assets();
//! ```
//!
//! Embed the build output and install the plugin:
//! ```rust,ignore
//! use autumn_plugin_sppl::{RustEmbed, SpplPlugin};
//!
//! #[derive(RustEmbed)]
//! #[folder = "$CARGO_MANIFEST_DIR/frontend/build"]
//! #[crate_path = "autumn_plugin_sppl::rust_embed"]
//! struct Assets;
//!
//! autumn_web::app()
//!     .plugin(SpplPlugin::<Assets>::new())
//!     .run()
//!     .await;
//! ```

use std::marker::PhantomData;

use autumn_web::{app::AppBuilder, plugin::Plugin, AppState};
use axum::{
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use http::{header, StatusCode};

// Re-export so users can derive RustEmbed and reference rust_embed support
// items without adding `sppl` or `rust-embed` to their own Cargo.toml.
pub use sppl::{rust_embed, RustEmbed};

/// Autumn plugin that serves a pre-built static site embedded at compile time.
///
/// Uses [sppl](https://github.com/neutral-engineering/sppl) to embed the build
/// output of a `SvelteKit` (or any static) site directly into the binary. Assets
/// are served via a fallback handler that follows `SvelteKit` `adapter-static`
/// path resolution, with transparent gzip pre-compression support.
///
/// # Example
///
/// ```rust,ignore
/// use autumn_plugin_sppl::{RustEmbed, SpplPlugin};
///
/// #[derive(RustEmbed)]
/// #[folder = "$CARGO_MANIFEST_DIR/frontend/build"]
/// #[crate_path = "autumn_plugin_sppl::rust_embed"]
/// struct Assets;
///
/// autumn_web::app()
///     .plugin(SpplPlugin::<Assets>::new())
///     .run()
///     .await;
/// ```
pub struct SpplPlugin<A: RustEmbed> {
    prefix: String,
    _marker: PhantomData<fn() -> A>,
}

impl<A: RustEmbed + 'static> SpplPlugin<A> {
    /// Create the plugin. Assets are served from the root (`/`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            prefix: String::new(),
            _marker: PhantomData,
        }
    }

    /// Mount the embedded assets under `prefix` instead of the root.
    ///
    /// The prefix must start with `/` and must not end with `/`,
    /// for example `.prefix("/app")`.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
}

impl<A: RustEmbed + 'static> Default for SpplPlugin<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: RustEmbed + Send + 'static> Plugin for SpplPlugin<A> {
    fn build(self, app: AppBuilder) -> AppBuilder {
        let router: axum::Router<AppState> =
            axum::Router::new().fallback(serve_asset::<A>);
        if self.prefix.is_empty() {
            app.merge(router)
        } else {
            app.nest(&self.prefix, router)
        }
    }
}

async fn serve_asset<A: RustEmbed + Send + 'static>(req: Request) -> Response {
    let path = req.uri().path();

    let Some(asset) = sppl::resolve::<A>(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_guess::from_path(&asset.path)
        .first_or_octet_stream()
        .to_string();

    let accepts_gzip = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.contains("gzip"));

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime);

    if asset.gzipped && accepts_gzip {
        builder
            .header(header::CONTENT_ENCODING, "gzip")
            .body(Body::from(asset.data.into_owned()))
            .expect("static response is valid")
    } else if asset.gzipped {
        match asset.decoded() {
            Ok(bytes) => builder
                .body(Body::from(bytes.into_owned()))
                .expect("static response is valid"),
            Err(e) => {
                tracing::warn!(path = path, error = %e, "failed to decompress sppl asset");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    } else {
        builder
            .body(Body::from(asset.data.into_owned()))
            .expect("static response is valid")
    }
}
