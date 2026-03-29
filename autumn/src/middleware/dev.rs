use axum::body::Body;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, EXPIRES, PRAGMA,
};
use axum::http::{HeaderValue, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;

pub(crate) const LIVE_RELOAD_PATH: &str = "/__autumn/live-reload";
const DEV_RELOAD_ENV: &str = "AUTUMN_DEV_RELOAD";
const DEV_RELOAD_STATE_ENV: &str = "AUTUMN_DEV_RELOAD_STATE";
const DEV_RELOAD_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate";

pub(crate) fn is_enabled() -> bool {
    std::env::var_os(DEV_RELOAD_ENV).is_some() && std::env::var_os(DEV_RELOAD_STATE_ENV).is_some()
}

pub(crate) async fn live_reload_state_handler() -> impl IntoResponse {
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

pub(crate) async fn disable_static_cache(request: Request<Body>, next: Next) -> Response<Body> {
    let is_static = is_static_path(request.uri().path());
    let mut response = next.run(request).await;
    if is_static {
        apply_no_store(response.headers_mut());
    }
    response
}

pub(crate) async fn inject_live_reload(request: Request<Body>, next: Next) -> Response<Body> {
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
  const endpoint = "{path}";
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
</script>"#,
        path = LIVE_RELOAD_PATH
    )
}
