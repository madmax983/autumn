//! Static build renderer.
//!
//! Renders `#[static_get]` routes through the Axum router and writes
//! the output HTML to a staging directory, then atomically swaps to
//! `dist/`. This is the engine behind `autumn build`.

use std::collections::HashMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use tower::ServiceExt;

use super::{ManifestEntry, StaticManifest, StaticRouteMeta, url_to_file_path};

/// Default number of routes rendered concurrently.
const DEFAULT_CONCURRENCY: usize = 8;

/// Errors that can occur during static rendering.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A route handler returned a non-2xx HTTP status.
    #[error("Route {path} returned HTTP {status} (expected 2xx)")]
    NonSuccessStatus {
        /// The URL path that failed.
        path: String,
        /// The HTTP status code returned.
        status: StatusCode,
    },

    /// Failed to read the response body from a route handler.
    #[error("Failed to read response body for {path}: {source}")]
    BodyRead {
        /// The URL path whose body could not be read.
        path: String,
        /// The underlying Axum error.
        source: axum::Error,
    },

    /// An I/O error occurred while writing files.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization error occurred while writing the manifest.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Render all static routes and write them to `dist_dir`.
///
/// Routes are rendered concurrently (up to `DEFAULT_CONCURRENCY` at a time)
/// using `buffer_unordered`.
///
/// 1. Renders to a staging directory (`{dist_dir}.staging`).
/// 2. On success, atomically renames staging -> dist.
/// 3. On failure, removes staging and returns the first error.
///
/// If `dist_dir` already exists, it is replaced.
///
/// # Errors
///
/// Returns [`BuildError`] if:
/// - Any route handler returns a non-2xx HTTP status.
/// - A response body cannot be read.
/// - An I/O error occurs while writing files or swapping directories.
/// - The manifest cannot be serialized to JSON.
///
/// # Panics
///
/// Panics if the Axum `Request` builder produces an invalid request
/// (should never happen with valid `StaticRouteMeta` paths) or if the
/// router's `oneshot` service returns an error (Axum routers are
/// infallible).
pub async fn render_static_routes(
    router: axum::Router,
    metas: &[StaticRouteMeta],
    dist_dir: &Path,
) -> Result<(), BuildError> {
    let staging = dist_dir.with_extension("staging");

    // Clean staging dir if it exists from a previous failed build
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    // Pre-create all subdirectories (avoids races between concurrent tasks)
    for meta in metas {
        let file_path = url_to_file_path(meta.path);
        let full_path = staging.join(&file_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Render concurrently
    let results: Vec<Result<(String, ManifestEntry), BuildError>> =
        futures::stream::iter(metas.iter().map(|meta| {
            let router = router.clone();
            let staging = staging.clone();
            async move {
                eprintln!("  Rendering {} ...", meta.path);

                let response = router
                    .oneshot(
                        Request::builder()
                            .uri(meta.path)
                            .body(Body::empty())
                            .expect("valid request"),
                    )
                    .await
                    .expect("router infallible");

                if !response.status().is_success() {
                    return Err(BuildError::NonSuccessStatus {
                        path: meta.path.to_owned(),
                        status: response.status(),
                    });
                }

                let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .map_err(|e| BuildError::BodyRead {
                        path: meta.path.to_owned(),
                        source: e,
                    })?;

                let file_path = url_to_file_path(meta.path);
                // staging dir pre-created above, just write
                let full_path = staging.join(&file_path);
                std::fs::write(&full_path, &body_bytes)?;

                Ok((
                    meta.path.to_owned(),
                    ManifestEntry {
                        file: file_path,
                        revalidate: meta.revalidate,
                    },
                ))
            }
        }))
        .buffer_unordered(DEFAULT_CONCURRENCY)
        .collect()
        .await;

    // Check for errors -- if any route failed, clean up and return first error
    let mut manifest_routes = HashMap::new();
    for result in results {
        match result {
            Ok((path, entry)) => {
                manifest_routes.insert(path, entry);
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        }
    }

    // Write manifest
    let manifest = StaticManifest {
        generated_at: timestamp_now(),
        autumn_version: env!("CARGO_PKG_VERSION").to_owned(),
        routes: manifest_routes,
    };
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(staging.join("manifest.json"), json)?;

    // Atomic swap: remove old dist, rename staging -> dist
    if dist_dir.exists() {
        std::fs::remove_dir_all(dist_dir)?;
    }
    std::fs::rename(&staging, dist_dir)?;

    Ok(())
}

/// Simple Unix-epoch timestamp (avoids pulling in chrono/time).
fn timestamp_now() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_gen::StaticRouteMeta;

    fn test_meta(path: &'static str, name: &'static str) -> StaticRouteMeta {
        StaticRouteMeta {
            path,
            name,
            revalidate: None,
        }
    }

    fn echo_router() -> axum::Router {
        axum::Router::new().fallback(axum::routing::get(|uri: axum::http::Uri| async move {
            format!("Hello from {}", uri.path())
        }))
    }

    #[tokio::test]
    async fn renders_single_route_to_dist() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let result =
            render_static_routes(echo_router(), &[test_meta("/about", "about")], &dist).await;
        assert!(result.is_ok(), "render failed: {:?}", result.err());
        let html = std::fs::read_to_string(dist.join("about/index.html")).unwrap();
        assert_eq!(html, "Hello from /about");
        let manifest = StaticManifest::load(&dist.join("manifest.json")).unwrap();
        assert_eq!(manifest.routes.len(), 1);
        assert!(manifest.routes.contains_key("/about"));
    }

    #[tokio::test]
    async fn renders_root_route() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let result = render_static_routes(echo_router(), &[test_meta("/", "index")], &dist).await;
        assert!(result.is_ok());
        let html = std::fs::read_to_string(dist.join("index.html")).unwrap();
        assert_eq!(html, "Hello from /");
    }

    #[tokio::test]
    async fn rejects_non_2xx_response() {
        let router =
            axum::Router::new().fallback(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") });
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let result = render_static_routes(router, &[test_meta("/about", "about")], &dist).await;
        assert!(result.is_err());
        assert!(!dist.exists(), "dist should not exist after failed build");
        let staging = dist.with_extension("staging");
        assert!(
            !staging.exists(),
            "staging dir should be cleaned up after failed build"
        );
    }

    #[tokio::test]
    async fn cleans_stale_dist_before_build() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("stale.html"), "old").unwrap();
        let result =
            render_static_routes(echo_router(), &[test_meta("/about", "about")], &dist).await;
        assert!(result.is_ok());
        assert!(!dist.join("stale.html").exists());
        assert!(dist.join("about/index.html").exists());
    }

    #[tokio::test]
    async fn renders_multiple_routes_concurrently() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let result = render_static_routes(
            echo_router(),
            &[
                test_meta("/", "index"),
                test_meta("/about", "about"),
                test_meta("/contact", "contact"),
            ],
            &dist,
        )
        .await;
        assert!(result.is_ok());
        let manifest = StaticManifest::load(&dist.join("manifest.json")).unwrap();
        assert_eq!(manifest.routes.len(), 3);
        // Verify all files exist
        assert!(dist.join("index.html").exists());
        assert!(dist.join("about/index.html").exists());
        assert!(dist.join("contact/index.html").exists());
    }
}
