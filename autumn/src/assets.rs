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

#[cfg(not(debug_assertions))]
use std::collections::HashMap;
#[cfg(not(debug_assertions))]
use std::sync::OnceLock;

/// On-disk format of `static/.autumn-manifest.json`.
#[cfg(not(debug_assertions))]
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
pub fn asset_url(path: &str) -> String {
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

/// Returns `true` if the URI path segment looks like a fingerprinted asset.
///
/// Fingerprinted files follow the naming convention
/// `<stem>.<8-hex-chars>.<ext>` (e.g. `autumn.a1b2c3d4.css`).
/// This is used by the static-file middleware to decide whether to emit
/// a long-lived immutable cache header.
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
}
