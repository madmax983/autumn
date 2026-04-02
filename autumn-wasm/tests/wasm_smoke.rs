#![cfg(target_arch = "wasm32")]

use autumn_wasm::{IslandRegistration, boot};
use wasm_bindgen_test::wasm_bindgen_test;

fn noop_mount(_: autumn_wasm::Element, _: String) {}

#[wasm_bindgen_test]
fn boot_accepts_registry_on_wasm_target() {
    let registry = [IslandRegistration::new(
        "counter",
        "counter-root",
        noop_mount,
    )];
    boot(&registry);
}
