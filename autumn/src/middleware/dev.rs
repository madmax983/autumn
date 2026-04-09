use axum::body::Body;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, EXPIRES, PRAGMA,
};
use axum::http::{HeaderValue, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;

pub const LIVE_RELOAD_PATH: &str = "/__autumn/live-reload";
const DEV_RELOAD_ENV: &str = "AUTUMN_DEV_RELOAD";
const DEV_RELOAD_STATE_ENV: &str = "AUTUMN_DEV_RELOAD_STATE";
const DEV_RELOAD_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate";

/// Checks if the development live-reload environment is active.
///
/// In the developer experience, it's crucial to tighten the feedback loop. This
/// function answers the core question: "Are we running in a watched development
/// session?" It exists to safely guard dev-only handlers (like the live-reload script
/// injector) from ever waking up in production.
///
/// Returns `true` if both the `AUTUMN_DEV_RELOAD` and
/// `AUTUMN_DEV_RELOAD_STATE` environment variables are present.
pub fn is_enabled() -> bool {
    std::env::var_os(DEV_RELOAD_ENV).is_some() && std::env::var_os(DEV_RELOAD_STATE_ENV).is_some()
}

pub async fn live_reload_state_handler() -> impl IntoResponse {
    let body =
        read_reload_state_body().unwrap_or_else(|| r#"{"version":0,"kind":"full"}"#.to_owned());
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

fn read_reload_state_body() -> Option<String> {
    let path = std::env::var_os(DEV_RELOAD_STATE_ENV)?;
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

fn live_reload_script() -> String {
    format!(
        r#"<script>
(() => {{
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
</script>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::EnvGuard;
    use axum::body::to_bytes;
    use axum::http::header::ACCEPT;
    use tower::ServiceExt;

    #[test]
    fn is_enabled_requires_both_env_vars() {
        {
            let _env = EnvGuard::set_many(&[(DEV_RELOAD_ENV, None), (DEV_RELOAD_STATE_ENV, None)]);
            assert!(!is_enabled());
        }

        {
            let _env =
                EnvGuard::set_many(&[(DEV_RELOAD_ENV, Some("1")), (DEV_RELOAD_STATE_ENV, None)]);
            assert!(!is_enabled());
        }

        {
            let _env = EnvGuard::set_many(&[
                (DEV_RELOAD_ENV, Some("1")),
                (DEV_RELOAD_STATE_ENV, Some("state.json")),
            ]);
            assert!(is_enabled());
        }
    }

    #[tokio::test]
    async fn live_reload_state_handler_defaults_when_state_missing() {
        let _env = EnvGuard::set_many(&[(DEV_RELOAD_ENV, Some("1")), (DEV_RELOAD_STATE_ENV, None)]);

        let response = live_reload_state_handler().await.into_response();
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
    async fn live_reload_state_handler_reads_file_when_present() {
        let tmp_file = tempfile::NamedTempFile::new().expect("failed to create temp file");
        let content = r#"{"version":42,"kind":"css"}"#;
        std::fs::write(tmp_file.path(), content).expect("failed to write to temp file");
        let _env = EnvGuard::set_many(&[
            (DEV_RELOAD_ENV, Some("1")),
            (DEV_RELOAD_STATE_ENV, tmp_file.path().to_str()),
        ]);

        let response = live_reload_state_handler().await.into_response();
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
        assert!(with_body.contains(LIVE_RELOAD_PATH));
        assert!(with_body.contains("</script></body>"));

        let html_shell = inject_snippet(b"<html><main>ok</main></html>");
        let html_shell = std::str::from_utf8(&html_shell).expect("utf-8 html");
        assert!(html_shell.ends_with("</script>"));

        let plain = inject_snippet(b"not html");
        assert_eq!(&plain[..], b"not html");
    }

    #[test]
    fn is_static_path_matches_root_and_nested_assets() {
        assert!(is_static_path("/static"));
        assert!(is_static_path("/static/css/autumn.css"));
        assert!(!is_static_path("/assets/autumn.css"));
    }
}
