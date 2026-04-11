//! `static_routes![]` collection macro.
//!
//! Collects `#[static_get]`-annotated handlers into a `Vec<StaticRouteMeta>`,
//! parallel to the `routes![]` and `tasks![]` macros.

use proc_macro2::TokenStream;

pub fn static_routes_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_static_meta_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_static_routes_macro() {
        let input = quote! { handler_a, pages::handler_b };
        let result = static_routes_macro(input);
        let result_str = result.to_string();

        assert!(result_str.contains("__autumn_static_meta_handler_a"));
        assert!(result_str.contains("pages :: __autumn_static_meta_handler_b"));
    }
}
