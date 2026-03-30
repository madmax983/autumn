mod manifest;
mod types;

#[cfg(feature = "maud")]
use maud::{Markup, html};

pub use manifest::{WasmIslandManifestEntry, WasmManifest};
pub use types::{ActionMeta, IslandMeta};

const DEFAULT_MANIFEST_PATH: &str = "target/autumn/wasm/manifest.json";

/// Load the optional WASM asset manifest from disk.
///
/// # Errors
///
/// Returns an error if the configured manifest path cannot be read or if
/// the JSON payload is invalid.
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
#[allow(clippy::needless_pass_by_value)]
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
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
        _guard: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let guard = ENV_LOCK.lock().expect("env lock poisoned");
            let previous = std::env::var_os(key);
            // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
            unsafe { std::env::set_var(key, value) };
            Self {
                key,
                previous,
                _guard: guard,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(self.key, previous) };
            } else {
                // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

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

    #[test]
    fn load_manifest_uses_env_override() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(
            temp.path(),
            r#"{"entry_js":"/static/client.js","entry_wasm":null,"islands":{}}"#,
        )
        .expect("write manifest");
        let _env = EnvGuard::set("AUTUMN_WASM_MANIFEST", temp.path());

        let manifest = load_manifest().expect("load manifest");

        assert_eq!(manifest.entry_js.as_deref(), Some("/static/client.js"));
        assert_eq!(manifest.entry_wasm, None);
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

    #[cfg(feature = "maud")]
    #[test]
    fn assets_render_entrypoints_from_manifest() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(
            temp.path(),
            r#"{"entry_js":"/static/client.js","entry_wasm":"/static/client_bg.wasm","islands":{}}"#,
        )
        .expect("write manifest");
        let _env = EnvGuard::set("AUTUMN_WASM_MANIFEST", temp.path());

        let markup = assets().into_string();

        assert!(markup.contains("src=\"/static/client.js\""));
        assert!(markup.contains("href=\"/static/client_bg.wasm\""));
    }

    #[cfg(feature = "maud")]
    #[test]
    fn assets_return_empty_markup_when_manifest_is_missing() {
        let missing = tempfile::tempdir()
            .expect("temp dir")
            .path()
            .join("missing-manifest.json");
        let _env = EnvGuard::set("AUTUMN_WASM_MANIFEST", &missing);

        assert_eq!(assets().into_string(), "");
    }
}
