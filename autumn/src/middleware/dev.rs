//! Live-reload and development middleware.
//!
//! When you run `autumn dev`, you don't want to manually refresh your browser every time
//! you tweak some CSS or HTML. This module provides the magical "live reload" functionality
//! that automatically injects a tiny JavaScript snippet into your HTML responses.
//!
//! It also explicitly disables caching for static files during development so you never
//! get stuck looking at yesterday's CSS.

use axum::body::Body;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, EXPIRES, PRAGMA,
};
use axum::http::{HeaderValue, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;

pub const LIVE_RELOAD_PATH: &str = "/__autumn/live-reload";
pub const LIVE_RELOAD_SCRIPT_PATH: &str = "/__autumn/live-reload.js";
const DEV_RELOAD_ENV: &str = "AUTUMN_DEV_RELOAD";
const DEV_RELOAD_STATE_ENV: &str = "AUTUMN_DEV_RELOAD_STATE";
const DEV_RELOAD_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate";

#[allow(dead_code)]
pub fn is_enabled() -> bool {
    is_enabled_with_env(&crate::config::OsEnv)
}

pub fn is_enabled_with_env(env: &dyn crate::config::Env) -> bool {
    env.var(DEV_RELOAD_ENV).is_ok() && env.var(DEV_RELOAD_STATE_ENV).is_ok()
}

pub async fn live_reload_state_handler() -> impl IntoResponse {
    let body = read_reload_state_body_with_env(&crate::config::OsEnv)
        .unwrap_or_else(|| r#"{"version":0,"kind":"full"}"#.to_owned());
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    apply_no_store(headers);
    response
}

/// Serves the live-reload polling client as an external JavaScript file.
///
/// The script is served as a same-origin `<script src="...">` rather than
/// injected inline so that it passes a `script-src 'self'` Content Security
/// Policy (the framework-default CSP). See [`LIVE_RELOAD_SCRIPT_PATH`].
pub async fn live_reload_script_handler() -> impl IntoResponse {
    let mut response = Response::new(Body::from(live_reload_script_body()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    apply_no_store(headers);
    response
}

pub async fn disable_static_cache(request: Request<Body>, next: Next) -> Response<Body> {
    let is_static = is_static_path(request.uri().path());
    let mut response = next.run(request).await;
    if is_static {
        apply_no_store(response.headers_mut());
    }
    response
}

pub async fn inject_live_reload(request: Request<Body>, next: Next) -> Response<Body> {
    let response = next.run(request).await;
    inject_live_reload_into_response(response).await
}

async fn inject_live_reload_into_response(response: Response<Body>) -> Response<Body> {
    if !is_html_response(&response) {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    // Dev-only middleware wraps normal HTML page responses.
    let body = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("live reload middleware should only wrap infallible HTML bodies");
    let updated = inject_snippet(&body);

    if updated == body {
        return Response::from_parts(parts, Body::from(body));
    }

    if let Ok(value) = HeaderValue::from_str(&updated.len().to_string()) {
        parts.headers.insert(CONTENT_LENGTH, value);
    } else {
        parts.headers.remove(CONTENT_LENGTH);
    }

    Response::from_parts(parts, Body::from(updated))
}

fn read_reload_state_body_with_env(env: &dyn crate::config::Env) -> Option<String> {
    let path = env.var(DEV_RELOAD_STATE_ENV).ok()?;
    std::fs::read_to_string(path).ok()
}

fn is_html_response(response: &Response<Body>) -> bool {
    if response.headers().contains_key(CONTENT_ENCODING) {
        return false;
    }

    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.contains("text/html"))
}

fn is_static_path(path: &str) -> bool {
    path == "/static" || path.starts_with("/static/")
}

fn apply_no_store(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(DEV_RELOAD_CACHE_CONTROL),
    );
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(EXPIRES, HeaderValue::from_static("0"));
}

fn inject_snippet(body: &[u8]) -> Vec<u8> {
    let html = String::from_utf8_lossy(body);
    let snippet = live_reload_script();

    if let Some(index) = html.rfind("</body>") {
        let mut html = html.into_owned();
        html.insert_str(index, &snippet);
        return html.into_bytes();
    }

    if html.contains("<html") || html.contains("</html>") {
        let mut html = html.into_owned();
        html.push_str(&snippet);
        return html.into_bytes();
    }

    body.to_vec()
}

/// Returns the `<script src=...>` tag injected into HTML responses.
///
/// The actual JavaScript lives at [`LIVE_RELOAD_SCRIPT_PATH`] so the tag
/// has no inline body and works under a same-origin-only CSP
/// (`script-src 'self'`). Tests and internal callers may still ask for
/// the bare tag via this function.
fn live_reload_script() -> String {
    format!(r#"<script src="{LIVE_RELOAD_SCRIPT_PATH}"></script>"#)
}

fn live_reload_script_body() -> String {
    format!(
        r#"(() => {{
  const endpoint = "{LIVE_RELOAD_PATH}";
  let version = null;
  let polling = false;

  function refreshStylesheets(nextVersion) {{
    let refreshed = 0;
    document.querySelectorAll('link[rel="stylesheet"]').forEach((link) => {{
      try {{
        const url = new URL(link.href, window.location.href);
        if (url.origin !== window.location.origin || !url.pathname.startsWith('/static/')) {{
          return;
        }}
        url.searchParams.set('__autumn_reload', String(nextVersion));
        link.href = url.toString();
        refreshed += 1;
      }} catch (_error) {{
      }}
    }});
    return refreshed > 0;
  }}

  async function poll() {{
    if (polling || document.visibilityState === 'hidden') {{
      return;
    }}
    polling = true;
    try {{
      const response = await fetch(endpoint + '?t=' + Date.now(), {{
        cache: 'no-store',
        headers: {{ Accept: 'application/json' }},
      }});
      if (!response.ok) {{
        return;
      }}

      const state = await response.json();
      if (version === null) {{
        version = state.version;
        return;
      }}
      if (state.version === version) {{
        return;
      }}

      version = state.version;
      if (state.kind === 'css' && refreshStylesheets(state.version)) {{
        return;
      }}

      window.location.reload();
    }} catch (_error) {{
    }} finally {{
      polling = false;
    }}
  }}

  window.addEventListener('pageshow', () => {{
    void poll();
  }});
  document.addEventListener('visibilitychange', () => {{
    if (document.visibilityState === 'visible') {{
      void poll();
    }}
  }});
  setInterval(() => {{
    void poll();
  }}, 700);
  void poll();
}})();
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MockEnv;
    use axum::body::to_bytes;
    use axum::http::header::ACCEPT;
    use tower::ServiceExt;

    #[test]
    fn is_enabled_requires_both_env_vars() {
        {
            let env = MockEnv::new();
            assert!(!is_enabled_with_env(&env));
        }

        {
            let env = MockEnv::new().with(DEV_RELOAD_ENV, "1");
            assert!(!is_enabled_with_env(&env));
        }

        {
            let env = MockEnv::new()
                .with(DEV_RELOAD_ENV, "1")
                .with(DEV_RELOAD_STATE_ENV, "state.json");
            assert!(is_enabled_with_env(&env));
        }
    }

    #[tokio::test]
    async fn read_reload_state_body_defaults_when_state_missing() {
        let env = MockEnv::new().with(DEV_RELOAD_ENV, "1");

        let body = read_reload_state_body_with_env(&env)
            .unwrap_or_else(|| r#"{"version":0,"kind":"full"}"#.to_owned());
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = StatusCode::OK;
        let headers = response.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        apply_no_store(headers);
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            DEV_RELOAD_CACHE_CONTROL
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(&body[..], br#"{"version":0,"kind":"full"}"#);
    }

    #[tokio::test]
    async fn read_reload_state_body_reads_file_when_present() {
        let tmp_file = tempfile::NamedTempFile::new().expect("failed to create temp file");
        let content = r#"{"version":42,"kind":"css"}"#;
        std::fs::write(tmp_file.path(), content).expect("failed to write to temp file");
        let env = MockEnv::new()
            .with(DEV_RELOAD_ENV, "1")
            .with(DEV_RELOAD_STATE_ENV, tmp_file.path().to_str().unwrap());

        let body = read_reload_state_body_with_env(&env).unwrap();
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = StatusCode::OK;
        let headers = response.headers_mut();
        apply_no_store(headers);
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            DEV_RELOAD_CACHE_CONTROL
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(&body[..], content.as_bytes());
    }

    #[tokio::test]
    async fn disable_static_cache_only_marks_static_paths() {
        let app = axum::Router::new()
            .route("/static/demo.txt", axum::routing::get(|| async { "demo" }))
            .route("/page", axum::routing::get(|| async { "page" }))
            .layer(axum::middleware::from_fn(disable_static_cache));

        let static_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/static/demo.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("static response");
        assert_eq!(
            static_response.headers().get(CACHE_CONTROL).unwrap(),
            DEV_RELOAD_CACHE_CONTROL
        );

        let page_response = app
            .oneshot(Request::builder().uri("/page").body(Body::empty()).unwrap())
            .await
            .expect("page response");
        assert!(page_response.headers().get(CACHE_CONTROL).is_none());
    }

    #[tokio::test]
    async fn inject_live_reload_into_response_updates_html_and_length() {
        let mut response = Response::new(Body::from("<html><body><main>ok</main></body></html>"));
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("1"));

        let response = inject_live_reload_into_response(response).await;
        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .expect("content-length header")
            .to_owned();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let html = std::str::from_utf8(&body).expect("utf-8 html");

        assert!(html.contains(LIVE_RELOAD_PATH));
        assert_eq!(content_length, body.len().to_string());
    }

    #[tokio::test]
    async fn inject_live_reload_into_response_skips_encoded_html() {
        let mut response = Response::new(Body::from("<html><body>ok</body></html>"));
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let response = inject_live_reload_into_response(response).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(&body[..], b"<html><body>ok</body></html>");
    }

    #[tokio::test]
    async fn inject_live_reload_middleware_leaves_non_html_responses_alone() {
        let app = axum::Router::new()
            .route(
                "/data",
                axum::routing::get(|| async {
                    ([(CONTENT_TYPE, "application/json")], r#"{"status":"ok"}"#)
                }),
            )
            .layer(axum::middleware::from_fn(inject_live_reload));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/data")
                    .header(ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("json response");
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }

    #[test]
    fn inject_snippet_inserts_before_body_or_appends_to_html_shell() {
        let with_body = inject_snippet(b"<html><body><main>ok</main></body></html>");
        let with_body = std::str::from_utf8(&with_body).expect("utf-8 html");
        assert!(with_body.contains(LIVE_RELOAD_SCRIPT_PATH));
        assert!(with_body.contains("</script></body>"));

        let html_shell = inject_snippet(b"<html><main>ok</main></html>");
        let html_shell = std::str::from_utf8(&html_shell).expect("utf-8 html");
        assert!(html_shell.ends_with("</script>"));

        let plain = inject_snippet(b"not html");
        assert_eq!(&plain[..], b"not html");
    }

    #[test]
    fn injected_snippet_uses_external_src_no_inline_js() {
        // The default Autumn CSP is `script-src 'self'` (no 'unsafe-inline'),
        // so the injected snippet must be a plain external-src <script> tag
        // with no inline JavaScript body — otherwise the browser will refuse
        // to execute it and live reload silently breaks.
        let injected = inject_snippet(b"<html><body>ok</body></html>");
        let html = std::str::from_utf8(&injected).expect("utf-8 html");

        assert!(
            html.contains(&format!(
                r#"<script src="{LIVE_RELOAD_SCRIPT_PATH}"></script>"#
            )),
            "expected external-src script tag, got: {html}"
        );
        // None of the inline script body should leak into the HTML.
        assert!(
            !html.contains("setInterval"),
            "inline JS leaked into HTML: {html}"
        );
        assert!(
            !html.contains("fetch("),
            "inline JS leaked into HTML: {html}"
        );
    }

    #[tokio::test]
    async fn live_reload_script_handler_serves_js_with_correct_content_type() {
        let response = live_reload_script_handler().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/javascript; charset=utf-8")
        );
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            DEV_RELOAD_CACHE_CONTROL
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let js = std::str::from_utf8(&body).expect("utf-8");
        // Sanity-check: handler returns the real polling client, not the
        // <script> wrapper tag.
        assert!(js.contains("fetch("), "expected polling client, got: {js}");
        assert!(
            !js.contains("<script"),
            "JS body must not contain HTML tags"
        );
    }

    #[test]
    fn inject_snippet_edge_cases() {
        // Multiple </body> tags: Should insert before the *last* </body> tag.
        let multi_body = inject_snippet(b"<html><body>text</body> <!-- </body> --> </html>");
        let multi_body_str = std::str::from_utf8(&multi_body).expect("utf-8");
        assert!(multi_body_str.ends_with("</script></body> --> </html>"));

        // <html tag with attributes.
        let html_attr = inject_snippet(b"<html lang=\"en\"><main>ok</main>");
        let html_attr_str = std::str::from_utf8(&html_attr).expect("utf-8");
        assert!(html_attr_str.ends_with("</script>"));

        // Only </html> tag.
        let html_close = inject_snippet(b"<div>content</div></html>");
        let html_close_str = std::str::from_utf8(&html_close).expect("utf-8");
        assert!(html_close_str.ends_with("</script>"));

        // Invalid UTF-8 sequence, but containing </body>.
        let invalid_utf8 = b"<html><body>\xFF\xFE</body></html>".to_vec();
        let invalid_result = inject_snippet(&invalid_utf8);
        // String::from_utf8_lossy Replaces invalid bytes with U+FFFD.
        assert!(String::from_utf8_lossy(&invalid_result).contains("</script></body></html>"));
    }

    #[test]
    fn is_static_path_matches_root_and_nested_assets() {
        assert!(is_static_path("/static"));
        assert!(is_static_path("/static/css/autumn.css"));
        assert!(!is_static_path("/assets/autumn.css"));
    }

    #[test]
    fn inject_snippet_edge_cases_empty_body() {
        let empty = inject_snippet(b"");
        assert_eq!(empty, b"");
    }

    #[test]
    fn inject_snippet_edge_cases_case_insensitivity() {
        // Our current implementation is case sensitive.
        // If a user has upper case tags, we shouldn't fail or panic, we just won't inject.
        let upper_body = inject_snippet(b"<HTML><BODY>ok</BODY></HTML>");
        let upper_result = std::str::from_utf8(&upper_body).expect("utf-8");
        assert_eq!(upper_result, "<HTML><BODY>ok</BODY></HTML>");
    }

    #[test]
    fn inject_snippet_edge_cases_malformed_but_matching() {
        let malformed = inject_snippet(b"<html<body>");
        let result = std::str::from_utf8(&malformed).expect("utf-8");
        assert!(result.ends_with("</script>"));
    }
}
