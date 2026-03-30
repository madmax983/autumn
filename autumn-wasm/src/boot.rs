use crate::IslandRegistration;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

pub fn boot(registry: Vec<IslandRegistration>) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };

        for island in registry {
            let selector = format!(
                "[data-autumn-island=\"{}\"][data-autumn-mount=\"{}\"]",
                island.name, island.mount_id
            );
            let Ok(nodes) = document.query_selector_all(&selector) else {
                continue;
            };
            for i in 0..nodes.length() {
                let Some(node) = nodes.item(i) else {
                    continue;
                };
                let Ok(root) = node.dyn_into::<web_sys::Element>() else {
                    continue;
                };

                let props = root
                    .query_selector("[data-autumn-props]")
                    .ok()
                    .flatten()
                    .map(|el| el.text_content().unwrap_or_default())
                    .unwrap_or_default();

                (island.mount)(root, props);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = registry;
    }
}
