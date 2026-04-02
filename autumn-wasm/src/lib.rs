pub mod action;
mod boot;
pub mod compose;
mod island;
pub mod prelude;
pub mod signal;

pub use boot::boot;
pub use compose::Composition;
pub use island::IslandRegistration;
pub use signal::{Signal, Subscription};

#[cfg(target_arch = "wasm32")]
pub type Element = web_sys::Element;

#[cfg(not(target_arch = "wasm32"))]
pub type Element = ();

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED: AtomicBool = AtomicBool::new(false);

    fn record_mount(_: crate::Element, _: String) {
        CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn prelude_reexports_boot_helpers() {
        CALLED.store(false, Ordering::SeqCst);
        let registry = [IslandRegistration::new(
            "counter",
            "counter-root",
            record_mount,
        )];

        boot(&registry);

        assert!(!CALLED.load(Ordering::SeqCst));
    }
}
