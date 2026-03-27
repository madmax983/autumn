//! Generic companion-function collection macro.
//!
//! Shared implementation for `routes![]`, `tasks![]`, and `static_routes![]`.
//! Each macro transforms a list of handler paths into companion function calls
//! with a specific prefix (e.g. `__autumn_route_info_`, `__autumn_static_meta_`).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Path, Token, punctuated::Punctuated};

/// Expand a comma-separated list of handler paths into `vec![companion_calls]`.
///
/// Each path's last segment is prefixed: `name` → `{prefix}{name}`.
/// Module-qualified paths like `users::list` become `users::{prefix}list`.
pub fn collect_companions(input: TokenStream, prefix: &str) -> TokenStream {
    if input.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }

    let paths: Punctuated<Path, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input) {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error(),
        };

    let calls: Vec<_> = paths
        .iter()
        .map(|path| {
            let mut companion = path.clone();
            if let Some(last) = companion.segments.last_mut() {
                last.ident = format_ident!("{}{}", prefix, last.ident);
            }
            quote! { #companion() }
        })
        .collect();

    quote! {
        vec![#(#calls),*]
    }
}
