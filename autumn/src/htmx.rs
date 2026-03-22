//! Embedded htmx JavaScript.
//!
//! htmx is embedded directly in the Autumn binary via [`include_bytes!`]
//! and served at `/static/js/htmx.min.js`. No CDN, no npm, no config.

/// htmx 2.x minified JavaScript, embedded at compile time.
pub const HTMX_JS: &[u8] = include_bytes!("../vendor/htmx.min.js");

/// htmx version string for diagnostics and cache busting.
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
            start.contains("htmx") || start.contains("function") || start.contains("("),
            "htmx.min.js doesn't look like JavaScript: {start}"
        );
    }

    #[test]
    fn htmx_version_matches_expected() {
        assert_eq!(HTMX_VERSION, "2.0.4");
    }
}
