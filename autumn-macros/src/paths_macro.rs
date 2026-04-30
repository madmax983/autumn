//! `paths![]` collection macro implementation.
//!
//! Generates a `pub mod paths { ... }` block that re-exports each handler's
//! `__autumn_path_{name}` companion function under a clean public name.
//!
//! # Usage
//!
//! ```ignore
//! paths![list_posts, show_post, create_post];
//! // Expands to:
//! // pub mod paths {
//! //     pub use super::__autumn_path_list_posts as list_posts;
//! //     pub use super::__autumn_path_show_post as show_post;
//! //     pub use super::__autumn_path_create_post as create_post;
//! // }
//! ```
//!
//! Module-qualified paths (`module::handler`) become:
//!
//! ```ignore
//! paths![posts::show_post];
//! // pub mod paths {
//! //     pub use super::posts::__autumn_path_show_post as show_post;
//! // }
//! ```

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Path, Token, punctuated::Punctuated};

/// Expand `paths![handler, ...]` into a `pub mod paths { ... }` block.
pub fn paths_macro(input: TokenStream) -> TokenStream {
    if input.is_empty() {
        return quote! {
            pub mod paths {}
        };
    }

    let handler_paths: Punctuated<Path, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input) {
            Ok(p) => p,
            Err(err) => return err.to_compile_error(),
        };

    let use_items: Vec<TokenStream> = handler_paths
        .iter()
        .map(|path| {
            // Derive the public alias name from the last segment.
            let last_ident = match path.segments.last() {
                Some(seg) => &seg.ident,
                None => return quote! { compile_error!("paths![] requires non-empty identifiers") },
            };
            let alias = last_ident.clone();

            // Build the companion path by prefixing the last segment with
            // `__autumn_path_`.  Module-qualified paths like `posts::show`
            // become `posts::__autumn_path_show`.
            let mut companion = path.clone();
            if let Some(last) = companion.segments.last_mut() {
                last.ident = format_ident!("__autumn_path_{}", last.ident);
            }

            // `pub use super::<companion> as <alias>` — purely item-level, valid
            // inside a `mod` block at any nesting depth.
            quote! {
                pub use super::#companion as #alias;
            }
        })
        .collect();

    quote! {
        pub mod paths {
            #(#use_items)*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn empty_input_generates_empty_module() {
        let result = paths_macro(quote! {});
        let s = result.to_string();
        assert!(s.contains("pub mod paths"));
    }

    #[test]
    fn single_handler_generates_use_item() {
        let result = paths_macro(quote! { show_post });
        let s = result.to_string();
        assert!(s.contains("__autumn_path_show_post"));
        assert!(s.contains("show_post"));
        assert!(s.contains("pub use"));
    }

    #[test]
    fn module_qualified_handler_uses_path() {
        let result = paths_macro(quote! { posts::show_post });
        let s = result.to_string();
        assert!(s.contains("__autumn_path_show_post"));
        assert!(s.contains("posts"));
    }

    #[test]
    fn multiple_handlers_all_appear() {
        let result = paths_macro(quote! { list_posts, show_post, create_post });
        let s = result.to_string();
        assert!(s.contains("__autumn_path_list_posts"));
        assert!(s.contains("__autumn_path_show_post"));
        assert!(s.contains("__autumn_path_create_post"));
    }

    #[test]
    fn trailing_comma_is_handled() {
        let result = paths_macro(quote! { show_post, list_posts, });
        let s = result.to_string();
        assert!(s.contains("__autumn_path_show_post"));
        assert!(s.contains("__autumn_path_list_posts"));
    }
}
