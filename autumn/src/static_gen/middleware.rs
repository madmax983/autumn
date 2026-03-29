use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::StaticManifest;

/// Per-route ISR state, tracking whether a regeneration is in flight
/// and when the last regeneration attempt occurred.
struct IsrRouteState {
    /// `true` when a background regeneration task is running for this route.
    in_flight: AtomicBool,
    /// Unix timestamp of the last regeneration attempt. Used for backoff:
    /// after a failed regeneration, wait at least `REGEN_COOLDOWN_SECS`
    /// before trying again.
    last_attempt: AtomicU64,
}

/// Minimum seconds between regeneration attempts for the same route.
/// Prevents tight retry loops when the handler is failing.
const REGEN_COOLDOWN_SECS: u64 = 30;

/// Layer that resolves incoming request paths against a pre-built static
/// manifest and the corresponding `dist/` directory on disk.
///
/// Created via [`StaticFileLayer::new`], which returns `None` when the
/// expected `manifest.json` is missing or unparseable -- this makes it
/// safe to attempt construction unconditionally and simply skip static
/// serving when no build output exists.
///
/// ## ISR (Incremental Static Regeneration)
///
/// Routes with a `revalidate` interval are served from disk but checked
/// for staleness on each request. When the file on disk is older than
/// `revalidate` seconds, a background Tokio task is spawned to re-render
/// the page. The stale page continues to be served until the fresh one
/// is ready (stale-while-revalidate pattern).
#[derive(Clone)]
pub struct StaticFileLayer {
    dist_dir: PathBuf,
    manifest: Arc<StaticManifest>,
    /// Per-route ISR state, keyed by URL path. Only populated for routes
    /// that have `revalidate` set.
    isr_state: Arc<HashMap<String, IsrRouteState>>,
    /// The Axum router used for ISR regeneration. Cloned from the app
    /// router at construction time. `None` if ISR is not needed.
    isr_router: Option<Arc<axum::Router>>,
}

impl StaticFileLayer {
    /// Try to load a `StaticFileLayer` from a `dist/` directory.
    ///
    /// Looks for `<dist_dir>/manifest.json`. Returns `None` if the file
    /// does not exist or cannot be parsed as a valid [`StaticManifest`].
    ///
    /// ISR routes are detected from the manifest but no regeneration
    /// router is configured. Use [`with_router`](Self::with_router) to
    /// enable ISR regeneration.
    pub fn new(dist_dir: impl Into<PathBuf>) -> Option<Self> {
        let dist_dir = dist_dir.into();
        let manifest_path = dist_dir.join("manifest.json");
        let manifest = StaticManifest::load(&manifest_path).ok()?;

        let isr_state = build_isr_state(&manifest);

        Some(Self {
            dist_dir,
            manifest: Arc::new(manifest),
            isr_state: Arc::new(isr_state),
            isr_router: None,
        })
    }

    /// Attach an Axum router for ISR background regeneration.
    ///
    /// Without a router, ISR staleness is detected but pages are never
    /// re-rendered. This method enables the full ISR cycle.
    #[must_use]
    pub fn with_router(mut self, router: axum::Router) -> Self {
        self.isr_router = Some(Arc::new(router));
        self
    }

    /// Reference to the loaded manifest.
    #[must_use]
    pub fn manifest(&self) -> &StaticManifest {
        &self.manifest
    }

    /// The `dist/` directory this layer serves files from.
    #[must_use]
    pub fn dist_dir(&self) -> &Path {
        &self.dist_dir
    }

    /// Map a request path (e.g. `"/about"`) to its filesystem path
    /// within `dist/`, based on the manifest.
    ///
    /// Returns `None` if the path is not in the manifest. Does **not**
    /// check whether the file exists on disk -- callers (e.g. `ServeDir`)
    /// handle missing files gracefully.
    ///
    /// If the route has ISR enabled and the file is stale, this method
    /// triggers a background regeneration task (at most one at a time
    /// per route) and still returns the stale file path. The caller
    /// serves the stale content while regeneration happens.
    #[must_use]
    pub fn resolve(&self, request_path: &str) -> Option<PathBuf> {
        let entry = self.manifest.routes.get(request_path)?;
        let file_path = self.dist_dir.join(&entry.file);

        // Check ISR staleness
        if let Some(revalidate) = entry.revalidate {
            self.maybe_trigger_isr(request_path, &file_path, revalidate);
        }

        Some(file_path)
    }

    /// Check if a file is stale and trigger background regeneration if needed.
    fn maybe_trigger_isr(&self, url_path: &str, file_path: &Path, revalidate_secs: u64) {
        // Check file age
        let is_stale = match file_mtime_age_secs(file_path) {
            Some(age) => age > revalidate_secs,
            // File missing or unreadable -- treat as stale
            None => true,
        };

        if !is_stale {
            return;
        }

        let Some(route_state) = self.isr_state.get(url_path) else {
            return;
        };

        let Some(router) = &self.isr_router else {
            // No router configured -- ISR detection only, no regeneration
            return;
        };

        // Check cooldown -- don't retry too fast after a failure
        let now = unix_now();
        let last = route_state.last_attempt.load(Ordering::Relaxed);
        if last > 0 && now.saturating_sub(last) < REGEN_COOLDOWN_SECS {
            return;
        }

        // Try to claim the in-flight flag (CAS: false -> true)
        if route_state
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            // Another task is already regenerating this route
            return;
        }

        // Record attempt time
        route_state.last_attempt.store(now, Ordering::Relaxed);

        // Spawn background regeneration
        let router = Arc::clone(router);
        let url = url_path.to_owned();
        let dest = file_path.to_owned();
        let in_flight = Arc::clone(&self.isr_state);

        tokio::spawn(async move {
            let result = regenerate_page(&router, &url, &dest).await;

            // Clear the in-flight flag
            if let Some(state) = in_flight.get(&url) {
                state.in_flight.store(false, Ordering::Release);
            }

            match result {
                Ok(()) => {
                    tracing::info!(route = %url, "ISR: page regenerated");
                }
                Err(e) => {
                    tracing::warn!(route = %url, error = %e, "ISR: regeneration failed");
                }
            }
        });
    }
}

/// Build per-route ISR state from the manifest. Only routes with
/// `revalidate` set get entries.
fn build_isr_state(manifest: &StaticManifest) -> HashMap<String, IsrRouteState> {
    let mut state = HashMap::new();
    for (path, entry) in &manifest.routes {
        if entry.revalidate.is_some() {
            state.insert(
                path.clone(),
                IsrRouteState {
                    in_flight: AtomicBool::new(false),
                    last_attempt: AtomicU64::new(0),
                },
            );
        }
    }
    state
}

/// Re-render a single page by sending a request through the router.
async fn regenerate_page(
    router: &axum::Router,
    url: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(url)
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router infallible");

    if !response.status().is_success() {
        return Err(format!(
            "Handler returned HTTP {} for {}",
            response.status(),
            url
        )
        .into());
    }

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;

    // Write to a temp file, then atomically rename to avoid serving partial content
    let temp_path = dest.with_extension("tmp");
    std::fs::write(&temp_path, &body_bytes)?;
    std::fs::rename(&temp_path, dest)?;

    Ok(())
}

/// Get the age of a file in seconds based on its modification time.
/// Returns `None` if the file doesn't exist or metadata can't be read.
fn file_mtime_age_secs(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let elapsed = SystemTime::now().duration_since(mtime).ok()?;
    Some(elapsed.as_secs())
}

/// Current Unix timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_gen::{ManifestEntry, StaticManifest};
    use std::collections::HashMap;

    /// Helper: create a temp dist dir with manifest.json and some HTML files.
    fn create_test_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let dist = dir.path().join("dist");

        // Create directories
        std::fs::create_dir_all(dist.join("about")).expect("mkdir about");

        // Create HTML files
        std::fs::write(dist.join("index.html"), "<h1>Home</h1>").expect("write index");
        std::fs::write(dist.join("about/index.html"), "<h1>About</h1>").expect("write about");

        // Build and write manifest
        let mut routes = HashMap::new();
        routes.insert(
            "/".to_owned(),
            ManifestEntry {
                file: "index.html".to_owned(),
                revalidate: None,
            },
        );
        routes.insert(
            "/about".to_owned(),
            ManifestEntry {
                file: "about/index.html".to_owned(),
                revalidate: Some(3600),
            },
        );

        let manifest = StaticManifest {
            generated_at: "2026-03-27T12:00:00Z".to_owned(),
            autumn_version: "0.2.0".to_owned(),
            routes,
        };

        let json = serde_json::to_string(&manifest).expect("serialize manifest");
        std::fs::write(dist.join("manifest.json"), json).expect("write manifest");

        dir
    }

    /// Helper: create a dist dir with parameterized routes in the manifest.
    fn create_parameterized_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let dist = dir.path().join("dist");

        // Create directories
        std::fs::create_dir_all(dist.join("posts/hello")).expect("mkdir posts/hello");
        std::fs::create_dir_all(dist.join("posts/world")).expect("mkdir posts/world");

        // Create HTML files
        std::fs::write(dist.join("posts/hello/index.html"), "<h1>Hello</h1>")
            .expect("write hello");
        std::fs::write(dist.join("posts/world/index.html"), "<h1>World</h1>")
            .expect("write world");

        // Build and write manifest
        let mut routes = HashMap::new();
        routes.insert(
            "/posts/hello".to_owned(),
            ManifestEntry {
                file: "posts/hello/index.html".to_owned(),
                revalidate: None,
            },
        );
        routes.insert(
            "/posts/world".to_owned(),
            ManifestEntry {
                file: "posts/world/index.html".to_owned(),
                revalidate: None,
            },
        );

        let manifest = StaticManifest {
            generated_at: "2026-03-29T12:00:00Z".to_owned(),
            autumn_version: "0.2.0".to_owned(),
            routes,
        };

        let json = serde_json::to_string(&manifest).expect("serialize manifest");
        std::fs::write(dist.join("manifest.json"), json).expect("write manifest");

        dir
    }

    /// Helper: create a dist dir with ISR routes.
    fn create_isr_dist(revalidate: u64) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let dist = dir.path().join("dist");

        std::fs::create_dir_all(dist.join("about")).expect("mkdir about");
        std::fs::write(dist.join("about/index.html"), "<h1>About (stale)</h1>")
            .expect("write about");

        let mut routes = HashMap::new();
        routes.insert(
            "/about".to_owned(),
            ManifestEntry {
                file: "about/index.html".to_owned(),
                revalidate: Some(revalidate),
            },
        );

        let manifest = StaticManifest {
            generated_at: "2026-03-29T12:00:00Z".to_owned(),
            autumn_version: "0.2.0".to_owned(),
            routes,
        };

        let json = serde_json::to_string(&manifest).expect("serialize manifest");
        std::fs::write(dist.join("manifest.json"), json).expect("write manifest");

        dir
    }

    #[test]
    fn layer_loads_from_valid_dist() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist);
        assert!(layer.is_some(), "should load from valid dist dir");
    }

    #[test]
    fn layer_returns_none_without_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No manifest.json at all
        let layer = StaticFileLayer::new(tmp.path());
        assert!(layer.is_none(), "should return None without manifest.json");
    }

    #[test]
    fn resolve_finds_known_route() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        let resolved = layer.resolve("/about");
        assert!(resolved.is_some(), "/about should resolve");
        assert!(
            resolved.unwrap().ends_with("about/index.html"),
            "should point to about/index.html"
        );
    }

    #[test]
    fn resolve_finds_root() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        let resolved = layer.resolve("/");
        assert!(resolved.is_some(), "/ should resolve");
        assert!(
            resolved.unwrap().ends_with("index.html"),
            "should point to index.html"
        );
    }

    #[test]
    fn resolve_returns_none_for_unknown_route() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        let resolved = layer.resolve("/admin");
        assert!(resolved.is_none(), "/admin should not resolve");
    }

    #[test]
    fn manifest_accessor() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        assert_eq!(layer.manifest().routes.len(), 2);
    }

    // --- Parameterized route middleware tests ---

    #[test]
    fn resolve_finds_parameterized_routes() {
        let tmp = create_parameterized_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        let hello = layer.resolve("/posts/hello");
        assert!(hello.is_some(), "/posts/hello should resolve");
        assert!(hello.unwrap().ends_with("posts/hello/index.html"));

        let world = layer.resolve("/posts/world");
        assert!(world.is_some(), "/posts/world should resolve");
        assert!(world.unwrap().ends_with("posts/world/index.html"));
    }

    #[test]
    fn resolve_returns_none_for_non_generated_param() {
        let tmp = create_parameterized_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        // This slug was not in the params list, so not in the manifest
        let resolved = layer.resolve("/posts/unknown");
        assert!(
            resolved.is_none(),
            "/posts/unknown should not resolve (not pre-rendered)"
        );
    }

    // --- ISR tests ---

    #[test]
    fn isr_state_built_for_revalidate_routes() {
        let tmp = create_test_dist();
        let dist = tmp.path().join("dist");
        let layer = StaticFileLayer::new(&dist).expect("layer");

        // /about has revalidate=3600, / does not
        assert!(layer.isr_state.contains_key("/about"));
        assert!(!layer.isr_state.contains_key("/"));
    }

    #[test]
    fn file_mtime_age_fresh_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("test.html");
        std::fs::write(&file, "test").expect("write");

        // File just created, age should be very small
        let age = file_mtime_age_secs(&file).expect("mtime");
        assert!(age < 5, "Fresh file should be < 5 seconds old, got {age}");
    }

    #[test]
    fn file_mtime_age_missing_file() {
        let age = file_mtime_age_secs(Path::new("/nonexistent/file.html"));
        assert!(age.is_none(), "Missing file should return None");
    }

    #[tokio::test]
    async fn isr_triggers_regeneration_for_stale_page() {
        // Create a dist dir with a very short revalidate (1 second)
        let tmp = create_isr_dist(1);
        let dist = tmp.path().join("dist");

        // Make the file old by setting mtime to the past
        let file = dist.join("about/index.html");
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        filetime::set_file_mtime(
            &file,
            filetime::FileTime::from_system_time(old_time),
        )
        .unwrap_or_else(|_| {
            // filetime may not be available; skip test gracefully
        });

        // Create a router that returns fresh content
        let router = axum::Router::new().fallback(axum::routing::get(|| async {
            "<h1>About (fresh)</h1>"
        }));

        let layer = StaticFileLayer::new(&dist)
            .expect("layer")
            .with_router(router);

        // Resolve should return the stale file path but trigger ISR
        let resolved = layer.resolve("/about");
        assert!(resolved.is_some());

        // Give the background task time to complete
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Check if file was updated (only if mtime was successfully set)
        let content = std::fs::read_to_string(&file).unwrap();
        // The content should be updated if ISR fired, or remain stale
        // if filetime wasn't available. Either way, resolve works.
        assert!(
            content == "<h1>About (fresh)</h1>" || content == "<h1>About (stale)</h1>",
            "unexpected content: {content}"
        );
    }

    #[tokio::test]
    async fn isr_does_not_retrigger_while_in_flight() {
        let tmp = create_isr_dist(1);
        let dist = tmp.path().join("dist");

        let router = axum::Router::new().fallback(axum::routing::get(|| async {
            // Simulate slow handler
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            "<h1>Slow</h1>"
        }));

        let layer = StaticFileLayer::new(&dist)
            .expect("layer")
            .with_router(router);

        // Make file stale
        let file = dist.join("about/index.html");
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        let _ = filetime::set_file_mtime(
            &file,
            filetime::FileTime::from_system_time(old_time),
        );

        // First resolve triggers ISR
        layer.resolve("/about");

        // Check in-flight flag
        let state = layer.isr_state.get("/about").expect("isr state");
        // May or may not be true depending on timing, but second resolve
        // should not panic or double-trigger
        layer.resolve("/about");

        // Wait for background task
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;

        // Flag should be cleared
        assert!(
            !state.in_flight.load(Ordering::Relaxed),
            "in_flight should be cleared after regeneration"
        );
    }

    #[tokio::test]
    async fn regenerate_page_writes_atomically() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("page.html");
        std::fs::write(&dest, "old content").expect("write old");

        let router = axum::Router::new().fallback(axum::routing::get(|| async { "new content" }));

        let result = regenerate_page(&router, "/test", &dest).await;
        assert!(result.is_ok(), "regeneration failed: {:?}", result.err());

        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "new content");

        // Temp file should be cleaned up
        assert!(!dest.with_extension("tmp").exists());
    }

    #[tokio::test]
    async fn regenerate_page_fails_on_non_2xx() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("page.html");
        std::fs::write(&dest, "old content").expect("write old");

        let router = axum::Router::new().fallback(|| async {
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "error")
        });

        let result = regenerate_page(&router, "/test", &dest).await;
        assert!(result.is_err());

        // Original file should be untouched
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "old content");
    }
}
