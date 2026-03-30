mod manifest;
mod types;

#[cfg(feature = "maud")]
use maud::{Markup, html};

pub use manifest::{WasmIslandManifestEntry, WasmManifest};
pub use types::{ActionMeta, IslandMeta};

const DEFAULT_MANIFEST_PATH: &str = "target/autumn/wasm/manifest.json";

pub fn load_manifest() -> crate::AutumnResult<WasmManifest> {
    let path =
        std::env::var("AUTUMN_WASM_MANIFEST").unwrap_or_else(|_| DEFAULT_MANIFEST_PATH.into());
    WasmManifest::load(std::path::Path::new(&path))
}

#[cfg(feature = "maud")]
#[must_use]
pub fn assets() -> Markup {
    let Ok(manifest) = load_manifest() else {
        return html! {};
    };

    html! {
        @if let Some(js) = manifest.entry_js {
            script type="module" src=(js) { }
        }
        @if let Some(wasm) = manifest.entry_wasm {
            link rel="modulepreload" href=(wasm);
        }
    }
}

#[cfg(feature = "maud")]
#[must_use]
pub fn island<P: serde::Serialize>(meta: IslandMeta, props: P, fallback: Markup) -> Markup {
    let encoded = serde_json::to_string(&props).unwrap_or_else(|_| "null".to_owned());
    html! {
        div data-autumn-island=(meta.name) data-autumn-mount=(meta.mount_id) {
            (fallback)
            script type="application/json" data-autumn-props {
                (encoded)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let manifest = WasmManifest {
            entry_js: Some("/static/autumn/client.js".into()),
            entry_wasm: Some("/static/autumn/client_bg.wasm".into()),
            islands: std::collections::BTreeMap::from([(
                "counter".into(),
                WasmIslandManifestEntry {
                    mount_id: "counter".into(),
                },
            )]),
        };

        let json = serde_json::to_string(&manifest).expect("serialize");
        let decoded: WasmManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, manifest);
    }

    #[cfg(feature = "maud")]
    #[test]
    fn island_renders_fallback_and_metadata() {
        let markup = island(
            IslandMeta {
                name: "counter",
                mount_id: "counter",
                props_type: "CounterProps",
            },
            serde_json::json!({ "initial": 3 }),
            html! { span { "3" } },
        )
        .into_string();

        assert!(markup.contains("data-autumn-island=\"counter\""));
        assert!(markup.contains("data-autumn-mount=\"counter\""));
        assert!(markup.contains("application/json"));
        assert!(markup.contains("<span>3</span>"));
    }

    #[cfg(feature = "maud")]
    #[test]
    fn island_escapes_props_json_for_script_tag_safety() {
        let markup = island(
            IslandMeta {
                name: "counter",
                mount_id: "counter",
                props_type: "CounterProps",
            },
            serde_json::json!({ "text": "</script><img src=x onerror=alert(1)>" }),
            html! {},
        )
        .into_string();

        assert!(markup.contains("&lt;/script&gt;&lt;img"));
        assert!(!markup.contains("</script><img"));
    }
}
