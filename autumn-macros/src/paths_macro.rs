//! `paths![]` collection macro implementation.
//!
//! Expands `paths![handler_a, handler_b]` into a `pub mod paths { … }` module
//! that re-exports each handler's `__autumn_path_*` companion function under
//! its short name, so callers can write `paths::handler_a(arg)` instead of
//! the internal `__autumn_path_handler_a(arg)`.
//!
//! Module-qualified paths are supported: `paths![posts::show, users::index]`
//! re-exports them as `paths::show` and `paths::index` respectively.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Path, Token, punctuated::Punctuated};

/// Expand `paths![handler_a, module::handler_b]` into a `pub mod paths { … }`.
pub fn paths_macro(input: TokenStream) -> TokenStream {
    if input.is_empty() {
        return quote! {
            pub mod paths {}
        };
    }

    let paths: Punctuated<Path, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input) {
            Ok(p) => p,
            Err(err) => return err.to_compile_error(),
        };

    let uses: Vec<_> = paths
        .iter()
        .map(|path| {
            // The short alias is the last segment's ident (e.g. `show` from `posts::show`).
            let alias = match path.segments.last() {
                Some(seg) => seg.ident.clone(),
                None => return quote! {},
            };

            // Build the companion path: prefix the last segment with `__autumn_path_`.
            let mut companion = path.clone();
            if let Some(last) = companion.segments.last_mut() {
                last.ident = format_ident!("__autumn_path_{}", last.ident);
            }

            quote! {
                pub use super::#companion as #alias;
            }
        })
        .collect();

    quote! {
        pub mod paths {
            #(#uses)*
        }
    }
}
