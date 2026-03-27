//! `static_routes![]` collection macro.
//!
//! Collects `#[static_get]`-annotated handlers into a `Vec<StaticRouteMeta>`,
//! parallel to the `routes![]` and `tasks![]` macros.

use proc_macro2::TokenStream;

pub fn static_routes_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_static_meta_")
}
