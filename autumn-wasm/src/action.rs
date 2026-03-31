use serde::{Serialize, de::DeserializeOwned};

#[cfg(target_arch = "wasm32")]
pub async fn post_json<I, O>(path: &str, input: &I) -> Result<O, String>
where
    I: Serialize,
    O: DeserializeOwned,
{
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let payload = serde_json::to_string(input).map_err(|error| error.to_string())?;
    let mut opts = web_sys::RequestInit::new();
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
        if let Some(document) = window.document() {
            if let Ok(Some(meta)) = document.query_selector("meta[name='csrf-token']") {
                if let Some(token) = meta.get_attribute("content") {
                    let _ = request.headers().set("x-csrf-token", &token);
                }
            }
        }

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

#[cfg(not(target_arch = "wasm32"))]
pub async fn post_json<I, O>(_path: &str, _input: &I) -> Result<O, String>
where
    I: Serialize,
    O: DeserializeOwned,
{
    Err("server action client is only available on wasm32".to_owned())
}

#[cfg(test)]
mod tests {
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
