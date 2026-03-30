mod boot;
mod island;

pub use boot::boot;
pub use island::IslandRegistration;

#[cfg(target_arch = "wasm32")]
pub type Element = web_sys::Element;

#[cfg(not(target_arch = "wasm32"))]
pub type Element = ();

pub mod prelude {
    pub use crate::{IslandRegistration, boot};
}

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
