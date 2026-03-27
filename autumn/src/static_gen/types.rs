use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Metadata for a route that should be statically generated at build time.
///
/// Used by the `#[static_get]` proc macro to register routes for the
/// static-site build step. The `revalidate` field controls ISR
/// (Incremental Static Regeneration): if set, the pre-rendered page
/// will be refreshed after the given number of seconds.
#[derive(Debug, Clone)]
pub struct StaticRouteMeta {
    /// The URL path pattern, e.g. `"/"` or `"/about"`.
    pub path: &'static str,
    /// The handler function name (used for diagnostics and manifest keys).
    pub name: &'static str,
    /// Optional ISR revalidation interval in seconds.
    /// `None` means the page is generated once and never refreshed.
    pub revalidate: Option<u64>,
}

/// Persistent manifest written by `autumn build` and read at runtime
/// by the static-file middleware.
///
/// Stored as JSON alongside the generated HTML files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticManifest {
    /// ISO-8601 timestamp of when the build ran.
    pub generated_at: String,
    /// Autumn framework version that produced this manifest.
    pub autumn_version: String,
    /// Map from URL path (e.g. `"/about"`) to the generated file entry.
    pub routes: HashMap<String, ManifestEntry>,
}

/// A single entry inside a [`StaticManifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative filesystem path to the generated HTML file
    /// (e.g. `"about/index.html"`).
    pub file: String,
    /// Optional ISR revalidation interval in seconds, copied from
    /// [`StaticRouteMeta::revalidate`].
    pub revalidate: Option<u64>,
}

impl StaticManifest {
    /// Load a manifest from a JSON file on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or contains invalid JSON.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&contents)?;
        Ok(manifest)
    }
}

/// Convert a URL path to the corresponding filesystem path for a
/// statically generated HTML file.
///
/// # Rules
///
/// | URL path | File path |
/// |----------|-----------|
/// | `/` | `index.html` |
/// | `/about` | `about/index.html` |
/// | `/about/` | `about/index.html` |
/// | `/posts/hello` | `posts/hello/index.html` |
#[must_use]
pub fn url_to_file_path(url_path: &str) -> String {
    let trimmed = url_path.trim_matches('/');
    if trimmed.is_empty() {
        "index.html".to_owned()
    } else {
        format!("{trimmed}/index.html")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn url_to_file_path_root() {
        assert_eq!(url_to_file_path("/"), "index.html");
    }

    #[test]
    fn url_to_file_path_simple() {
        assert_eq!(url_to_file_path("/about"), "about/index.html");
    }

    #[test]
    fn url_to_file_path_nested() {
        assert_eq!(url_to_file_path("/posts/hello"), "posts/hello/index.html");
    }

    #[test]
    fn url_to_file_path_trailing_slash() {
        assert_eq!(url_to_file_path("/about/"), "about/index.html");
    }

    #[test]
    fn manifest_roundtrip() {
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

        // Serialize to JSON
        let json = serde_json::to_string(&manifest).expect("serialize");

        // Write to a temp file, then load back via StaticManifest::load
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("manifest.json");
        {
            let mut f = std::fs::File::create(&file_path).expect("create file");
            f.write_all(json.as_bytes()).expect("write");
        }

        let loaded = StaticManifest::load(&file_path).expect("load");

        assert_eq!(loaded.generated_at, "2026-03-27T12:00:00Z");
        assert_eq!(loaded.autumn_version, "0.2.0");
        assert_eq!(loaded.routes.len(), 2);

        let root_entry = loaded.routes.get("/").expect("root route");
        assert_eq!(root_entry.file, "index.html");
        assert!(root_entry.revalidate.is_none());

        let about_entry = loaded.routes.get("/about").expect("about route");
        assert_eq!(about_entry.file, "about/index.html");
        assert_eq!(about_entry.revalidate, Some(3600));
    }

    #[test]
    fn static_route_meta_clone() {
        let meta = StaticRouteMeta {
            path: "/test",
            name: "test_handler",
            revalidate: Some(60),
        };
        // Verify Clone works by cloning into a Vec (prevents redundant_clone lint)
        let items = vec![meta.clone()];
        assert_eq!(items[0].path, "/test");
        assert_eq!(items[0].name, "test_handler");
        assert_eq!(items[0].revalidate, Some(60));
    }
}
