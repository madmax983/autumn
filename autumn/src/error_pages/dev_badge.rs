//! Next.js-style dev error badge overlay.
//!
//! In dev mode, error responses get a floating badge injected into the HTML
//! that shows error details: status code, error type, message, and optional
//! stack trace. The badge uses **inline CSS only** (no Tailwind) so it works
//! even when the CSS build pipeline is broken.
//!
//! The badge is **never** injected in production.

use maud::{Markup, PreEscaped, html};

/// Context for the dev error badge.
#[derive(Debug, Clone)]
pub struct DevBadgeContext {
    /// HTTP status code.
    pub status_code: u16,
    /// Status reason phrase (e.g., "Not Found").
    pub status_reason: String,
    /// Error message.
    pub message: String,
    /// Request path.
    pub path: String,
    /// Request ID.
    pub request_id: Option<String>,
    /// Optional file/line info if available.
    pub source_location: Option<String>,
    /// Optional query string.
    pub query: Option<String>,
    /// Scrubbed request headers.
    pub headers: serde_json::Value,
}

/// Generate the dev error badge HTML snippet.
///
/// This returns a self-contained HTML fragment with inline CSS and no
/// JavaScript, so it works under Autumn's default `script-src 'self'` CSP.
/// It should be injected just before `</body>` in HTML error responses.
///
/// Uses inline CSS (not Tailwind) so it works even if the CSS build fails.
/// Styled like Next.js: dark overlay, monospace font, red accent, expandable.
pub fn dev_error_badge_html(ctx: &DevBadgeContext) -> Markup {
    let status = ctx.status_code;
    let reason = &ctx.status_reason;
    let message = &ctx.message;
    let path = &ctx.path;
    let request_id = ctx.request_id.as_deref().unwrap_or("n/a");
    let source_loc = ctx.source_location.as_deref().unwrap_or("");
    let query = ctx.query.as_deref().unwrap_or("n/a");
    let headers = ctx.headers.to_string();

    html! {
        (PreEscaped(DEV_BADGE_STYLES))
        div #autumn-dev-error-badge {
            input #autumn-dev-badge-toggle type="checkbox" class="autumn-dev-toggle";

            // Collapsed badge (always visible)
            label #autumn-dev-badge-collapsed
                for="autumn-dev-badge-toggle"
                tabindex="0"
            {
                span class="autumn-dev-badge-dot" {}
                span class="autumn-dev-badge-code" { (status) }
                span class="autumn-dev-badge-label" { (reason) }
            }

            // Expanded overlay
            div #autumn-dev-badge-expanded style="display:none" {
                div class="autumn-dev-overlay-header" {
                    div class="autumn-dev-overlay-title" {
                        span class="autumn-dev-badge-dot" {}
                        (status) " " (reason)
                    }
                    label class="autumn-dev-overlay-close"
                        for="autumn-dev-badge-toggle"
                        role="button"
                        aria-label="Close error details"
                    {
                        "\u{00d7}"
                    }
                }
                div class="autumn-dev-overlay-body" {
                    div class="autumn-dev-overlay-section" {
                        div class="autumn-dev-overlay-label" { "Message" }
                        div class="autumn-dev-overlay-value" { (message) }
                    }
                    div class="autumn-dev-overlay-section" {
                        div class="autumn-dev-overlay-label" { "Path" }
                        div class="autumn-dev-overlay-value autumn-dev-mono" { (path) }
                    }
                    div class="autumn-dev-overlay-section" {
                        div class="autumn-dev-overlay-label" { "Request ID" }
                        div class="autumn-dev-overlay-value autumn-dev-mono" { (request_id) }
                    }
                    div class="autumn-dev-overlay-section" {
                        div class="autumn-dev-overlay-label" { "Query" }
                        div class="autumn-dev-overlay-value autumn-dev-mono" { (query) }
                    }
                    div class="autumn-dev-overlay-section" {
                        div class="autumn-dev-overlay-label" { "Headers" }
                        div class="autumn-dev-overlay-value autumn-dev-mono" { (headers) }
                    }
                    @if !source_loc.is_empty() {
                        div class="autumn-dev-overlay-section" {
                            div class="autumn-dev-overlay-label" { "Source" }
                            div class="autumn-dev-overlay-value autumn-dev-mono" { (source_loc) }
                        }
                    }
                }
            }
        }
    }
}

/// All inline CSS for the dev badge. Uses a unique prefix to avoid
/// colliding with application styles.
const DEV_BADGE_STYLES: &str = r#"<style>
#autumn-dev-error-badge {
    position: fixed;
    bottom: 16px;
    left: 16px;
    z-index: 99999;
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    font-size: 13px;
    line-height: 1.5;
}
.autumn-dev-toggle {
    position: absolute;
    opacity: 0;
    pointer-events: none;
}
#autumn-dev-badge-toggle:not(:checked) ~ #autumn-dev-badge-expanded {
    display: none;
}
#autumn-dev-badge-toggle:checked ~ #autumn-dev-badge-collapsed {
    display: none;
}
#autumn-dev-badge-collapsed {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    background: #1a1a2e;
    border: 1px solid #e53e3e;
    border-radius: 8px;
    color: #e2e8f0;
    cursor: pointer;
    box-shadow: 0 4px 24px rgba(0,0,0,0.4);
    transition: background 0.15s;
    user-select: none;
}
#autumn-dev-badge-collapsed:hover {
    background: #2d2d4a;
}
.autumn-dev-badge-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #e53e3e;
    flex-shrink: 0;
}
.autumn-dev-badge-code {
    font-weight: 700;
    color: #fc8181;
}
.autumn-dev-badge-label {
    color: #a0aec0;
    font-size: 12px;
}
#autumn-dev-badge-expanded {
    display: flex;
    flex-direction: column;
    width: 480px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 100px);
    background: #1a1a2e;
    border: 1px solid #e53e3e;
    border-radius: 12px;
    color: #e2e8f0;
    box-shadow: 0 8px 32px rgba(0,0,0,0.6);
    overflow: hidden;
}
.autumn-dev-overlay-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: #16162a;
    border-bottom: 1px solid #2d2d4a;
}
.autumn-dev-overlay-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 700;
    color: #fc8181;
    font-size: 14px;
}
.autumn-dev-overlay-close {
    background: none;
    border: none;
    color: #a0aec0;
    font-size: 20px;
    cursor: pointer;
    padding: 0 4px;
    line-height: 1;
}
.autumn-dev-overlay-close:hover {
    color: #e2e8f0;
}
.autumn-dev-overlay-body {
    padding: 16px;
    overflow-y: auto;
}
.autumn-dev-overlay-section {
    margin-bottom: 12px;
}
.autumn-dev-overlay-section:last-child {
    margin-bottom: 0;
}
.autumn-dev-overlay-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: #718096;
    margin-bottom: 4px;
}
.autumn-dev-overlay-value {
    color: #e2e8f0;
    word-break: break-word;
}
.autumn-dev-mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 12px;
    color: #a0aec0;
}
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> DevBadgeContext {
        DevBadgeContext {
            status_code: 404,
            status_reason: "Not Found".into(),
            message: "page missing".into(),
            path: "/test".into(),
            request_id: Some("req-abc".into()),
            source_location: None,
            query: None,
            headers: serde_json::json!({}),
        }
    }

    #[test]
    fn badge_contains_status_code() {
        let html = dev_error_badge_html(&test_ctx());
        let s = html.into_string();
        assert!(s.contains("404"));
        assert!(s.contains("Not Found"));
    }

    #[test]
    fn badge_contains_message() {
        let html = dev_error_badge_html(&test_ctx());
        let s = html.into_string();
        assert!(s.contains("page missing"));
    }

    #[test]
    fn badge_contains_request_id() {
        let html = dev_error_badge_html(&test_ctx());
        let s = html.into_string();
        assert!(s.contains("req-abc"));
    }

    #[test]
    fn badge_uses_inline_css() {
        let html = dev_error_badge_html(&test_ctx());
        let s = html.into_string();
        assert!(s.contains("<style>"), "badge must use inline CSS");
        // Should NOT reference Tailwind classes for its own styling
        assert!(
            s.contains("#autumn-dev-error-badge"),
            "badge uses namespaced CSS selectors"
        );
    }

    #[test]
    fn badge_does_not_use_inline_javascript_handlers() {
        let html = dev_error_badge_html(&test_ctx());
        let s = html.into_string();
        assert!(!s.contains("onclick="));
        assert!(!s.contains("<script"));
        assert!(s.contains("autumn-dev-badge-toggle"));
    }

    #[test]
    fn badge_shows_source_location_when_present() {
        let mut ctx = test_ctx();
        ctx.source_location = Some("src/main.rs:42".into());
        let html = dev_error_badge_html(&ctx);
        let s = html.into_string();
        assert!(s.contains("src/main.rs:42"));
    }

    #[test]
    fn badge_hides_source_section_when_absent() {
        let ctx = test_ctx();
        let html = dev_error_badge_html(&ctx);
        let s = html.into_string();
        assert!(!s.contains("Source"), "no source section without location");
    }
}
