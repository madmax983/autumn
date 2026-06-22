//! Fingerprinted asset pipeline for cache-busted static file delivery.
//!
//! In release builds, [`asset_url`] resolves a logical asset path to a
//! content-hashed URL using the manifest written by `autumn build --release`.
//! In development, it returns the plain `/static/...` URL so edits are
//! immediately visible without a build step.
//!
//! # Usage
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//!
//! html! {
//!     link rel="stylesheet" href=(asset_url("css/autumn.css"));
//!     // debug:   /static/css/autumn.css
//!     // release: /static/css/autumn.a1b2c3d4.css
//! }
//! ```
//!
//! # Manifest
//!
//! The manifest is written by `autumn build --release` to
//! `static/.autumn-manifest.json`.  It maps logical paths (relative to
//! `static/`) to fingerprinted paths:
//!
//! ```json
//! {
//!   "version": "1",
//!   "files": {
//!     "css/autumn.css": "css/autumn.a1b2c3d4.css"
//!   }
//! }
//! ```

#[cfg(any(not(debug_assertions), feature = "embed-assets"))]
use std::collections::HashMap;
#[cfg(any(not(debug_assertions), feature = "embed-assets"))]
use std::sync::OnceLock;

/// On-disk format of `static/.autumn-manifest.json`.
#[cfg(any(not(debug_assertions), feature = "embed-assets"))]
#[derive(Debug, serde::Deserialize)]
struct AssetManifest {
    files: HashMap<String, String>,
}

#[cfg(not(debug_assertions))]
static ASSET_MANIFEST: OnceLock<Option<AssetManifest>> = OnceLock::new();

#[cfg(not(debug_assertions))]
fn load_manifest() -> &'static Option<AssetManifest> {
    ASSET_MANIFEST.get_or_init(|| {
        let manifest_path =
            crate::app::project_dir("static", &crate::config::OsEnv).join(".autumn-manifest.json");
        let contents = std::fs::read_to_string(manifest_path).ok()?;
        serde_json::from_str(&contents).ok()
    })
}

// ── Embedded assets (feature = "embed-assets") ──────────────────────────────
//
// When the app registers an embedded `static/` tree via
// [`AppBuilder::embedded_static`](crate::app::AppBuilder::embedded_static),
// the binary is fully self-contained: `asset_url`/`is_manifest_asset` resolve
// against the manifest baked into the binary (no disk read) and `/static/*`
// is served from the embedded bytes. Both the manifest and the files come from
// the *same* build, so fingerprint-vs-manifest drift is impossible.
//
// The embedded path is active regardless of `debug_assertions`: it engages only
// once a dir is registered, so dev builds (which never register one) are
// unaffected and keep serving from disk for hot-reload.

/// A `static/` directory embedded into the binary at compile time.
///
/// Produced by [`embed_static!`](crate::embed_static) and handed to
/// [`AppBuilder::embedded_static`](crate::app::AppBuilder::embedded_static).
#[cfg(feature = "embed-assets")]
#[derive(Clone, Copy)]
pub struct EmbeddedStaticDir(pub &'static include_dir::Dir<'static>);

#[cfg(feature = "embed-assets")]
impl std::fmt::Debug for EmbeddedStaticDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedStaticDir")
            .field("path", &self.0.path())
            .finish()
    }
}

#[cfg(feature = "embed-assets")]
static EMBEDDED_STATIC: OnceLock<EmbeddedStaticDir> = OnceLock::new();

#[cfg(feature = "embed-assets")]
static EMBEDDED_MANIFEST: OnceLock<Option<AssetManifest>> = OnceLock::new();

/// Register the embedded `static/` tree as the process-wide asset source.
///
/// Parses the embedded `.autumn-manifest.json` so [`asset_url`] and
/// [`is_manifest_asset`] resolve against it. Called by the framework during
/// `AppBuilder::run` before the router is built; calling it more than once is a
/// no-op (the first registration wins).
#[cfg(feature = "embed-assets")]
pub fn register_embedded_static(dir: EmbeddedStaticDir) {
    let _ = EMBEDDED_STATIC.set(dir);
    let _ = EMBEDDED_MANIFEST.set(
        dir.0
            .get_file(".autumn-manifest.json")
            .and_then(include_dir::File::contents_utf8)
            .and_then(|s| serde_json::from_str::<AssetManifest>(s).ok()),
    );
}

/// The registered embedded `static/` dir, if [`register_embedded_static`] ran.
#[cfg(feature = "embed-assets")]
#[must_use]
pub fn embedded_static_dir() -> Option<EmbeddedStaticDir> {
    EMBEDDED_STATIC.get().copied()
}

/// Look up a logical asset path in the embedded manifest, returning the
/// fingerprinted path when present.
#[cfg(feature = "embed-assets")]
fn embedded_manifest_lookup(path: &str) -> Option<String> {
    EMBEDDED_MANIFEST.get()?.as_ref()?.files.get(path).cloned()
}

/// `true` if `rel_path` is a fingerprinted value in the embedded manifest.
#[cfg(feature = "embed-assets")]
fn embedded_is_manifest_asset(rel_path: &str) -> bool {
    EMBEDDED_MANIFEST
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|m| m.files.values().any(|v| v == rel_path))
}

/// Return the URL for a static asset, fingerprinted in release builds.
///
/// - **Debug builds** (`cargo run` / `autumn dev`): returns `/static/{path}`
///   with no manifest lookup so edits are always visible immediately.
/// - **Release builds** (`cargo build --release`): looks up `path` in the
///   manifest produced by `autumn build --release` and returns the
///   content-hashed URL (e.g. `/static/css/autumn.a1b2c3d4.css`).
///   Falls back to `/static/{path}` when the manifest is absent or the path
///   is not listed, so the app keeps serving without fingerprinted assets.
///
/// # Example
///
/// ```rust,ignore
/// link rel="stylesheet" href=(asset_url("css/autumn.css"));
/// // debug:   /static/css/autumn.css
/// // release: /static/css/autumn.a1b2c3d4.css
/// ```
#[must_use]
pub fn asset_url(path: &str) -> String {
    // When an embedded `static/` tree is registered (single-binary build), the
    // embedded manifest is the *only* source of truth — assets are served from
    // the binary, so a miss means the asset isn't fingerprinted and should use
    // the plain embedded path. Never fall through to a disk sidecar manifest,
    // which could otherwise point at a hashed file that was never baked in.
    #[cfg(feature = "embed-assets")]
    {
        if embedded_static_dir().is_some() {
            return embedded_manifest_lookup(path)
                .map_or_else(|| format!("/static/{path}"), |fp| format!("/static/{fp}"));
        }
    }
    #[cfg(debug_assertions)]
    {
        format!("/static/{path}")
    }
    #[cfg(not(debug_assertions))]
    {
        if let Some(manifest) = load_manifest() {
            if let Some(fingerprinted) = manifest.files.get(path) {
                return format!("/static/{fingerprinted}");
            }
        }
        format!("/static/{path}")
    }
}

/// Returns `true` if `rel_path` (the portion of the URL after `/static/`) is
/// listed as a fingerprinted value in the release asset manifest.
///
/// Gating the `immutable` cache header on manifest membership rather than
/// filename pattern alone ensures that user-authored assets whose names
/// happen to match `<stem>.<8hex>.<ext>` (e.g. `vendor.deadbeef.js`) are
/// never given a year-long cache lifetime.
///
/// Always returns `false` in debug builds — the manifest does not exist there.
// Not `const fn`: the body branches on build profile/feature, and the
// release/embedded arms perform non-const lookups.
#[allow(clippy::missing_const_for_fn)]
pub(crate) fn is_manifest_asset(rel_path: &str) -> bool {
    // When an embedded `static/` tree is registered, its manifest is the sole
    // authority for the immutable-cache decision (and is reachable in debug
    // builds too); never consult a disk sidecar manifest in that case.
    #[cfg(feature = "embed-assets")]
    {
        if embedded_static_dir().is_some() {
            return embedded_is_manifest_asset(rel_path);
        }
    }
    #[cfg(not(debug_assertions))]
    {
        load_manifest()
            .as_ref()
            .is_some_and(|m| m.files.values().any(|v| v == rel_path))
    }
    #[cfg(debug_assertions)]
    {
        let _ = rel_path;
        false
    }
}

/// Returns `true` if the URI path segment looks like a fingerprinted asset
/// by filename convention (`<stem>.<8-hex-chars>.<ext>`).
///
/// Only used in tests; the cache middleware uses [`is_manifest_asset`] for
/// production cache decisions.
#[cfg(test)]
pub(crate) fn is_fingerprinted_path(uri_path: &str) -> bool {
    let filename = uri_path.rsplit('/').next().unwrap_or("");
    let parts: Vec<&str> = filename.split('.').collect();
    if parts.len() < 3 {
        return false;
    }
    let hash_candidate = parts[parts.len() - 2];
    hash_candidate.len() == 8
        && hash_candidate
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Cache-control policy for `/static/*` responses.
///
/// Fingerprinted assets (members of the manifest, embedded or on-disk) get a
/// year-long `immutable` lifetime; everything else gets `must-revalidate` so
/// returning visitors always pick up the latest file after a deploy. Manifest
/// membership — rather than filename pattern — gates the `immutable` policy so a
/// user-authored `vendor.deadbeef.js` is never frozen for a year.
///
/// Applied as a middleware layer over both the on-disk (`ServeDir`) and the
/// embedded static-serving paths so the policy is identical regardless of where
/// the bytes come from.
pub async fn asset_cache_control(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path().to_owned();
    let mut resp = next.run(req).await;
    if path.starts_with("/static/") && resp.status().is_success() {
        let is_immutable = path.strip_prefix("/static/").is_some_and(is_manifest_asset);
        let header = if is_immutable {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=0, must-revalidate"
        };
        resp.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static(header),
        );
    }
    resp
}

/// Best-effort `Content-Type` for an embedded asset, derived from its
/// extension. Covers the closed set of asset types Autumn apps ship; unknown
/// extensions fall back to `application/octet-stream`.
#[cfg(feature = "embed-assets")]
#[must_use]
pub(crate) fn content_type_for(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
    match ext.as_str() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Serve a single file from the registered embedded `static/` tree.
///
/// Returns `404` for traversal attempts, dotfiles (so the embedded
/// `.autumn-manifest.json` is never exposed), missing files, or when no
/// embedded dir is registered.
#[cfg(feature = "embed-assets")]
fn embedded_response(rel_path: &str) -> axum::response::Response {
    use axum::response::IntoResponse;

    if rel_path
        .split('/')
        .any(|seg| seg.is_empty() || seg == ".." || seg.starts_with('.'))
    {
        return http::StatusCode::NOT_FOUND.into_response();
    }
    let Some(dir) = embedded_static_dir() else {
        return http::StatusCode::NOT_FOUND.into_response();
    };
    dir.0.get_file(rel_path).map_or_else(
        || http::StatusCode::NOT_FOUND.into_response(),
        |file| {
            (
                [(http::header::CONTENT_TYPE, content_type_for(rel_path))],
                file.contents().to_vec(),
            )
                .into_response()
        },
    )
}

/// Axum handler serving `/static/{*path}` from the embedded tree.
#[cfg(feature = "embed-assets")]
pub(crate) async fn serve_embedded(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> axum::response::Response {
    embedded_response(&path)
}

/// A standalone router that serves `/static/*` from the registered embedded
/// tree with the [`asset_cache_control`] policy applied.
///
/// The framework wires the embedded handler directly into the typed
/// application router; this helper exposes the same serving behavior as a
/// self-contained `Router` for tests and embedding into custom setups.
#[cfg(feature = "embed-assets")]
pub fn embedded_static_router() -> axum::Router {
    axum::Router::new()
        .route("/static/{*path}", axum::routing::get(serve_embedded))
        .layer(axum::middleware::from_fn(asset_cache_control))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_url_returns_static_prefix() {
        let url = asset_url("css/autumn.css");
        assert!(
            url.starts_with("/static/"),
            "url must have /static/ prefix: {url}"
        );
        assert!(
            url.contains("autumn.css"),
            "url must contain asset name: {url}"
        );
    }

    #[test]
    fn fingerprinted_path_detected() {
        assert!(is_fingerprinted_path("/static/css/autumn.a1b2c3d4.css"));
        assert!(is_fingerprinted_path("/static/js/app.00000000.js"));
        assert!(is_fingerprinted_path("/static/img/logo.deadbeef.png"));
    }

    #[test]
    fn non_fingerprinted_paths_rejected() {
        assert!(!is_fingerprinted_path("/static/css/autumn.css"));
        assert!(!is_fingerprinted_path("/static/js/htmx.min.js"));
        assert!(!is_fingerprinted_path("/static/img/logo.png"));
        // Hash too short
        assert!(!is_fingerprinted_path("/static/css/autumn.abc.css"));
        // Hash too long
        assert!(!is_fingerprinted_path("/static/css/autumn.a1b2c3d4e5.css"));
        // Hash contains uppercase (not hex-lowercase)
        assert!(!is_fingerprinted_path("/static/css/autumn.A1B2C3D4.css"));
        // No extension
        assert!(!is_fingerprinted_path("/static/file.a1b2c3d4"));
    }

    #[cfg(feature = "embed-assets")]
    #[test]
    fn content_type_covers_common_assets() {
        assert_eq!(content_type_for("css/app.css"), "text/css; charset=utf-8");
        assert_eq!(
            content_type_for("js/app.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for("img/logo.svg"), "image/svg+xml");
        assert_eq!(content_type_for("img/logo.png"), "image/png");
        assert_eq!(content_type_for("fonts/inter.woff2"), "font/woff2");
        assert_eq!(content_type_for("favicon.ico"), "image/x-icon");
        // Unknown / extensionless fall back to octet-stream.
        assert_eq!(content_type_for("data.bin"), "application/octet-stream");
        assert_eq!(content_type_for("LICENSE"), "application/octet-stream");
    }
}
