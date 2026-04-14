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

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use http::header::{HeaderName, HeaderValue};
use std::convert::Infallible;

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

/// Extractor for htmx request headers.
///
/// Extracts the standard `hx-*` headers sent by htmx requests, enabling
/// conditional rendering (e.g. sending back partials instead of full pages).
///
/// See <https://htmx.org/reference/#request_headers> for more details.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct HxRequest {
    /// Indicates that the request is via htmx (`HX-Request`)
    pub is_htmx: bool,
    /// The id of the target element if provided (`HX-Target`)
    pub target: Option<String>,
    /// The id of the triggered element if provided (`HX-Trigger`)
    pub trigger: Option<String>,
    /// The name of the triggered element if provided (`HX-Trigger-Name`)
    pub trigger_name: Option<String>,
    /// The current URL of the browser (`HX-Current-URL`)
    pub current_url: Option<String>,
    /// `true` if the request is for history restoration after a miss (`HX-History-Restore-Request`)
    pub history_restore_request: bool,
    /// The user response to an hx-prompt (`HX-Prompt`)
    pub prompt: Option<String>,
    /// `true` if the request is via an element using hx-boost (`HX-Boosted`)
    pub boosted: bool,
}

impl<S> FromRequestParts<S> for HxRequest
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header_str = |name: &'static str| -> Option<String> {
            parts
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(ToString::to_string)
        };
        let header_bool =
            |name: &'static str| -> bool { parts.headers.get(name).is_some_and(|v| v == "true") };

        Ok(Self {
            is_htmx: header_bool("hx-request"),
            target: header_str("hx-target"),
            trigger: header_str("hx-trigger"),
            trigger_name: header_str("hx-trigger-name"),
            current_url: header_str("hx-current-url"),
            history_restore_request: header_bool("hx-history-restore-request"),
            prompt: header_str("hx-prompt"),
            boosted: header_bool("hx-boosted"),
        })
    }
}

/// Extension trait for adding htmx response headers to any `IntoResponse` type.
///
/// This trait provides a fluent API for controlling htmx behavior from the server.
/// If an invalid header value is provided, it is gracefully ignored.
///
/// See <https://htmx.org/reference/#response_headers> for more details.
pub trait HxResponseExt: IntoResponse + Sized {
    /// Pushes a new URL into the history stack (`HX-Push-Url`).
    fn hx_push_url(self, url: &str) -> Response {
        append_hx_header(self, "hx-push-url", url)
    }

    /// Triggers a client-side redirect (`HX-Redirect`).
    fn hx_redirect(self, url: &str) -> Response {
        append_hx_header(self, "hx-redirect", url)
    }

    /// Tells the client to do a full page refresh (`HX-Refresh`).
    fn hx_refresh(self) -> Response {
        append_hx_header(self, "hx-refresh", "true")
    }

    /// Replaces the current URL in the location bar (`HX-Replace-Url`).
    fn hx_replace_url(self, url: &str) -> Response {
        append_hx_header(self, "hx-replace-url", url)
    }

    /// Specifies how the response will be swapped (`HX-Reswap`).
    fn hx_reswap(self, swap: &str) -> Response {
        append_hx_header(self, "hx-reswap", swap)
    }

    /// Specifies the target element to update (`HX-Retarget`).
    fn hx_retarget(self, target: &str) -> Response {
        append_hx_header(self, "hx-retarget", target)
    }

    /// Triggers client-side events (`HX-Trigger`).
    fn hx_trigger(self, event: &str) -> Response {
        append_hx_header(self, "hx-trigger", event)
    }

    /// Triggers client-side events after the settle step (`HX-Trigger-After-Settle`).
    fn hx_trigger_after_settle(self, event: &str) -> Response {
        append_hx_header(self, "hx-trigger-after-settle", event)
    }

    /// Triggers client-side events after the swap step (`HX-Trigger-After-Swap`).
    fn hx_trigger_after_swap(self, event: &str) -> Response {
        append_hx_header(self, "hx-trigger-after-swap", event)
    }
}

impl<T: IntoResponse> HxResponseExt for T {}

fn append_hx_header<T: IntoResponse>(response: T, name: &'static str, value: &str) -> Response {
    let mut res = response.into_response();
    if let Ok(v) = HeaderValue::from_str(value) {
        res.headers_mut().insert(HeaderName::from_static(name), v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[test]
    #[allow(clippy::const_is_empty)]
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

    #[tokio::test]
    async fn hx_request_extractor_parses_headers() -> Result<(), axum::http::Error> {
        let req = Request::builder()
            .header("hx-request", "true")
            .header("hx-target", "my-div")
            .header("hx-trigger", "btn")
            .header("hx-trigger-name", "btn-name")
            .header("hx-current-url", "http://example.com")
            .header("hx-history-restore-request", "true")
            .header("hx-prompt", "yes")
            .header("hx-boosted", "true")
            .body(())?;
        let (mut parts, ()) = req.into_parts();

        let hx = HxRequest::from_request_parts(&mut parts, &())
            .await
            .expect("infallible");

        assert!(hx.is_htmx);
        assert_eq!(hx.target.as_deref(), Some("my-div"));
        assert_eq!(hx.trigger.as_deref(), Some("btn"));
        assert_eq!(hx.trigger_name.as_deref(), Some("btn-name"));
        assert_eq!(hx.current_url.as_deref(), Some("http://example.com"));
        assert!(hx.history_restore_request);
        assert_eq!(hx.prompt.as_deref(), Some("yes"));
        assert!(hx.boosted);
        Ok(())
    }

    #[tokio::test]
    async fn hx_response_ext_adds_headers() {
        use axum::response::IntoResponse;
        let response = "hello"
            .hx_push_url("/new-url")
            .hx_redirect("/login")
            .hx_refresh()
            .hx_replace_url("/old-url")
            .hx_reswap("innerHTML")
            .hx_retarget("#target")
            .hx_trigger("my-event")
            .hx_trigger_after_settle("settled-event")
            .hx_trigger_after_swap("swapped-event")
            .into_response();

        let headers = response.headers();
        assert_eq!(headers.get("hx-push-url").unwrap(), "/new-url");
        assert_eq!(headers.get("hx-redirect").unwrap(), "/login");
        assert_eq!(headers.get("hx-refresh").unwrap(), "true");
        assert_eq!(headers.get("hx-replace-url").unwrap(), "/old-url");
        assert_eq!(headers.get("hx-reswap").unwrap(), "innerHTML");
        assert_eq!(headers.get("hx-retarget").unwrap(), "#target");
        assert_eq!(headers.get("hx-trigger").unwrap(), "my-event");
        assert_eq!(
            headers.get("hx-trigger-after-settle").unwrap(),
            "settled-event"
        );
        assert_eq!(
            headers.get("hx-trigger-after-swap").unwrap(),
            "swapped-event"
        );
    }

    #[tokio::test]
    async fn hx_request_extractor_handles_missing_headers() -> Result<(), axum::http::Error> {
        let req = Request::builder().body(())?;
        let (mut parts, ()) = req.into_parts();

        let hx = HxRequest::from_request_parts(&mut parts, &())
            .await
            .expect("infallible");

        assert!(!hx.is_htmx);
        assert_eq!(hx.target, None);
        assert_eq!(hx.trigger, None);
        assert_eq!(hx.trigger_name, None);
        assert_eq!(hx.current_url, None);
        assert!(!hx.history_restore_request);
        assert_eq!(hx.prompt, None);
        assert!(!hx.boosted);
        Ok(())
    }
}
