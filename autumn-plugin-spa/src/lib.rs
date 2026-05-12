//! Embed a pre-built single-page application directly into the binary and
//! serve it from an Autumn application.
//!
//! Works with any static site generator that emits a directory of files:
//! `Vite`, `SvelteKit` (with `adapter-static`), Astro, `Next.js` (static
//! export), plain HTML, or a hand-rolled build. The build output is baked
//! into the binary at compile time via [`rust_embed`], so the server ships
//! as a single self-contained executable.
//!
//! # Setup
//!
//! Add to `Cargo.toml`:
//! ```toml
//! [dependencies]
//! autumn-plugin-spa = { git = "https://github.com/madmax983/autumn" }
//! ```
//!
//! Embed the build output and install the plugin:
//! ```rust,ignore
//! use autumn_plugin_spa::{RustEmbed, SpaPlugin};
//!
//! #[derive(RustEmbed)]
//! #[folder = "$CARGO_MANIFEST_DIR/frontend/dist"]
//! #[crate_path = "autumn_plugin_spa::rust_embed"]
//! struct Assets;
//!
//! autumn_web::app()
//!     .plugin(SpaPlugin::<Assets>::new())
//!     .run()
//!     .await;
//! ```
//!
//! # Compression
//!
//! If a sibling `.gz` file exists for any asset (e.g. `index.html.gz`),
//! it is preferred and served with `Content-Encoding: gzip` to clients
//! that accept gzip. Clients that don't get the bytes decompressed on
//! the fly. Pre-gzip at build time however you like (a `build.rs` walker,
//! a `npm` script, etc.) — this plugin doesn't care who produced the `.gz`.

use std::borrow::Cow;
use std::marker::PhantomData;

use autumn_web::{app::AppBuilder, plugin::Plugin, AppState};
use axum::{
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use http::{header, StatusCode};

// Re-export so users can derive `RustEmbed` and reference its support items
// without adding `rust-embed` to their own `Cargo.toml`.
pub use rust_embed::RustEmbed;
#[doc(hidden)]
pub use rust_embed;

/// Autumn plugin that serves a pre-built SPA embedded at compile time.
///
/// Assets are served via a fallback handler that follows the standard SPA
/// resolution chain: exact path → `<path>.html` → `<path>/index.html` →
/// `index.html` (SPA fallback). Pre-gzipped `.gz` variants are preferred
/// when present and the client accepts gzip encoding.
pub struct SpaPlugin<A: RustEmbed> {
    prefix: String,
    _marker: PhantomData<fn() -> A>,
}

impl<A: RustEmbed + 'static> SpaPlugin<A> {
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

impl<A: RustEmbed + 'static> Default for SpaPlugin<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: RustEmbed + Send + 'static> Plugin for SpaPlugin<A> {
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

struct Resolved {
    path: String,
    data: Cow<'static, [u8]>,
    gzipped: bool,
}

/// Look up a file in an embedded bundle.
///
/// Tries: exact path → `<path>.html` → `<path>/index.html` → `index.html`.
/// For each candidate, a `.gz` variant is preferred if present.
fn resolve<A: RustEmbed>(path: &str) -> Option<Resolved> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let trimmed = path.trim_end_matches('/');

    let candidates = [
        path.to_string(),
        format!("{trimmed}.html"),
        format!("{trimmed}/index.html"),
        "index.html".to_string(),
    ];

    for candidate in candidates {
        let gz = format!("{candidate}.gz");
        if let Some(file) = A::get(&gz) {
            return Some(Resolved {
                path: candidate,
                data: file.data,
                gzipped: true,
            });
        }
        if let Some(file) = A::get(&candidate) {
            return Some(Resolved {
                path: candidate,
                data: file.data,
                gzipped: false,
            });
        }
    }
    None
}

fn decode_gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::with_capacity(data.len() * 4);
    flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

async fn serve_asset<A: RustEmbed + Send + 'static>(req: Request) -> Response {
    let path = req.uri().path();

    let Some(asset) = resolve::<A>(path) else {
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
        match decode_gzip(&asset.data) {
            Ok(bytes) => builder
                .body(Body::from(bytes))
                .expect("static response is valid"),
            Err(e) => {
                tracing::warn!(path = path, error = %e, "failed to decompress embedded asset");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    } else {
        builder
            .body(Body::from(asset.data.into_owned()))
            .expect("static response is valid")
    }
}
