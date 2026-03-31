//! Embedded htmx JavaScript.
//!
//! htmx is embedded directly in the Autumn binary via [`include_bytes!`]
//! and served at `/static/js/htmx.min.js`. No CDN, no npm, no build
//! step required.
//!
//! The framework automatically mounts a route handler that serves this
//! file with immutable caching headers. Reference it in your HTML
//! templates:
//!
//! ```html
//! <script src="/static/js/htmx.min.js"></script>
//! ```

/// htmx 2.x minified JavaScript, embedded at compile time.
///
/// This is the raw byte content of the minified htmx library. It is
/// served automatically by the framework at `/static/js/htmx.min.js`
/// with `Cache-Control: public, max-age=31536000, immutable`.
pub const HTMX_JS: &[u8] = include_bytes!("../vendor/htmx.min.js");

/// htmx version string for diagnostics and cache busting.
///
/// Corresponds to the version of the embedded htmx JS file.
/// Re-exported at the crate root as [`HTMX_VERSION`].
pub const HTMX_VERSION: &str = "2.0.4";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn htmx_js_is_not_empty() {
        assert!(!HTMX_JS.is_empty(), "htmx.min.js should not be empty");
    }

    #[test]
    fn htmx_js_looks_like_javascript() {
        let start = std::str::from_utf8(&HTMX_JS[..50]).expect("htmx should be valid UTF-8");
        assert!(
            start.contains("htmx") || start.contains("function") || start.contains('('),
            "htmx.min.js doesn't look like JavaScript: {start}"
        );
    }

    #[test]
    fn htmx_version_matches_expected() {
        assert_eq!(HTMX_VERSION, "2.0.4");
    }
}
