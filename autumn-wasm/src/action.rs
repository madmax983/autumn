use serde::{Serialize, de::DeserializeOwned};

#[must_use]
pub fn parse_cookie_value(cookie_header: &str, cookie_name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let mut pieces = part.trim().splitn(2, '=');
        let key = pieces.next()?.trim();
        let value = pieces.next()?.trim();
        if key == cookie_name && !value.is_empty() {
            Some(value.to_owned())
        } else {
            None
        }
    })
}

#[cfg(target_arch = "wasm32")]
/// Send a JSON POST request to a generated Autumn server action endpoint.
///
/// # Errors
///
/// Returns an error string when request construction fails, the network request
/// fails, the response status is not successful, or JSON decoding fails.
pub async fn post_json<I, O>(path: &str, input: &I) -> Result<O, String>
where
    I: Serialize,
    O: DeserializeOwned,
{
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let payload = serde_json::to_string(input).map_err(|error| error.to_string())?;
    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(web_sys::RequestMode::SameOrigin);
    opts.set_credentials(web_sys::RequestCredentials::SameOrigin);
    opts.set_body(&wasm_bindgen::JsValue::from_str(&payload));

    let request = web_sys::Request::new_with_str_and_init(path, &opts)
        .map_err(|error| format!("request build failed: {error:?}"))?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|error| format!("header set failed: {error:?}"))?;

    if let Some(window) = web_sys::window() {
        apply_csrf_headers(&window, &request);

        let response = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|error| format!("network failed: {error:?}"))?;
        let response: web_sys::Response = response
            .dyn_into()
            .map_err(|_| "response cast failed".to_owned())?;

        if !response.ok() {
            return Err(format!("request failed with status {}", response.status()));
        }

        let json = JsFuture::from(
            response
                .json()
                .map_err(|error| format!("json body read failed: {error:?}"))?,
        )
        .await
        .map_err(|error| format!("json promise failed: {error:?}"))?;

        serde_wasm_bindgen::from_value(json).map_err(|error| error.to_string())
    } else {
        Err("window unavailable".to_owned())
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_csrf_headers(window: &web_sys::Window, request: &web_sys::Request) {
    use wasm_bindgen::JsCast;

    let mut header_name = "x-csrf-token".to_owned();
    let mut token = None;

    if let Some(document) = window.document() {
        if let Ok(Some(meta)) = document.query_selector("meta[name='autumn-csrf-header']") {
            if let Some(value) = meta.get_attribute("content") {
                if !value.trim().is_empty() {
                    header_name = value;
                }
            }
        }

        if let Ok(Some(meta)) = document.query_selector("meta[name='csrf-token']") {
            token = non_empty(meta.get_attribute("content"));
        }
        if token.is_none() {
            if let Ok(Some(meta)) = document.query_selector("meta[name='autumn-csrf-token']") {
                token = non_empty(meta.get_attribute("content"));
            }
        }

        if token.is_none() {
            let cookie_name = document
                .query_selector("meta[name='autumn-csrf-cookie']")
                .ok()
                .flatten()
                .and_then(|meta| meta.get_attribute("content"))
                .unwrap_or_else(|| "autumn-csrf".to_owned());

            if let Ok(html_document) = document.dyn_into::<web_sys::HtmlDocument>() {
                if let Ok(cookie_header) = html_document.cookie() {
                    token = parse_cookie_value(&cookie_header, &cookie_name);
                }
            }
        }
    }

    if let Some(token) = token {
        let _ = request.headers().set(&header_name, &token);
    }
}

#[cfg(target_arch = "wasm32")]
fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

#[cfg(not(target_arch = "wasm32"))]
/// Non-wasm fallback for action client transport.
///
/// # Errors
///
/// Always returns an error because browser transport is unavailable on
/// non-wasm targets.
#[allow(clippy::future_not_send, clippy::unused_async)]
pub async fn post_json<I, O>(_path: &str, _input: &I) -> Result<O, String>
where
    I: Serialize,
    O: DeserializeOwned,
{
    Err("server action client is only available on wasm32".to_owned())
}

#[cfg(test)]
mod tests {
    use super::parse_cookie_value;

    #[test]
    fn parse_cookie_value_extracts_named_cookie() {
        let cookies = "session=abc; autumn-csrf=token-123; mode=dark";
        assert_eq!(
            parse_cookie_value(cookies, "autumn-csrf"),
            Some("token-123".to_owned())
        );
    }

    #[test]
    fn parse_cookie_value_handles_missing_cookie() {
        assert_eq!(parse_cookie_value("session=abc", "autumn-csrf"), None);
    }

    #[test]
    fn parse_cookie_value_rejects_empty_values() {
        assert_eq!(
            parse_cookie_value("autumn-csrf=; session=abc", "autumn-csrf"),
            None
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn post_json_returns_error_on_non_wasm_target() {
        let result = super::post_json::<serde_json::Value, serde_json::Value>(
            "/_autumn/actions/demo",
            &serde_json::json!({"ok": true}),
        )
        .await;

        assert!(result.is_err());
    }
}
