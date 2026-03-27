use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::StaticManifest;

/// Layer that resolves incoming request paths against a pre-built static
/// manifest and the corresponding `dist/` directory on disk.
///
/// Created via [`StaticFileLayer::new`], which returns `None` when the
/// expected `manifest.json` is missing or unparseable -- this makes it
/// safe to attempt construction unconditionally and simply skip static
/// serving when no build output exists.
#[derive(Clone)]
pub struct StaticFileLayer {
    dist_dir: PathBuf,
    manifest: Arc<StaticManifest>,
}

impl StaticFileLayer {
    /// Try to load a `StaticFileLayer` from a `dist/` directory.
    ///
    /// Looks for `<dist_dir>/manifest.json`. Returns `None` if the file
    /// does not exist or cannot be parsed as a valid [`StaticManifest`].
    pub fn new(dist_dir: impl Into<PathBuf>) -> Option<Self> {
        let dist_dir = dist_dir.into();
        let manifest_path = dist_dir.join("manifest.json");
        let manifest = StaticManifest::load(&manifest_path).ok()?;
        Some(Self {
            dist_dir,
            manifest: Arc::new(manifest),
        })
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

    /// Check whether `request_path` (e.g. `"/about"`) maps to a known
    /// manifest route **and** the corresponding file exists on disk.
    ///
    /// Returns the absolute path to the file if both conditions hold,
    /// or `None` otherwise.
    #[must_use]
    pub fn resolve(&self, request_path: &str) -> Option<PathBuf> {
        let entry = self.manifest.routes.get(request_path)?;
        let file_path = self.dist_dir.join(&entry.file);
        if file_path.exists() {
            Some(file_path)
        } else {
            None
        }
    }
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
}
