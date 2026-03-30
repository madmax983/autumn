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
