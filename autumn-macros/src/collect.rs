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
            // Emit a dummy use of the original path so that typos surface
            // errors on the user's identifier, not just the generated macro prefix.
            quote! {
                {
                    #[allow(clippy::let_unit_value, clippy::no_effect)]
                    {
                        let _autumn_dummy = &#path;
                    }
                    #companion()
                }
            }
        })
        .collect();

    quote! {
        vec![#(#calls),*]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn tokens_to_string(tokens: &TokenStream) -> String {
        tokens.to_string()
    }

    #[test]
    fn test_collect_companions_empty() {
        let input = quote! {};
        let result = collect_companions(input, "__prefix_");
        assert_eq!(
            tokens_to_string(&result),
            tokens_to_string(&quote! { ::std::vec::Vec::new() })
        );
    }

    #[test]
    fn test_collect_companions_single() {
        let input = quote! { handler };
        let result = collect_companions(input, "__prefix_");
        assert_eq!(
            tokens_to_string(&result),
            tokens_to_string(
                &quote! { vec![{ #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &handler; } __prefix_handler() }] }
            )
        );
    }

    #[test]
    fn test_collect_companions_multiple() {
        let input = quote! { a, b, c };
        let result = collect_companions(input, "__prefix_");
        assert_eq!(
            tokens_to_string(&result),
            tokens_to_string(
                &quote! { vec![{ #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &a; } __prefix_a() }, { #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &b; } __prefix_b() }, { #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &c; } __prefix_c() }] }
            )
        );
    }

    #[test]
    fn test_collect_companions_module_path() {
        let input = quote! { users::list, auth::login };
        let result = collect_companions(input, "__prefix_");
        assert_eq!(
            tokens_to_string(&result),
            tokens_to_string(
                &quote! { vec![{ #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &users::list; } users::__prefix_list() }, { #[allow(clippy::let_unit_value, clippy::no_effect)] { let _autumn_dummy = &auth::login; } auth::__prefix_login() }] }
            )
        );
    }

    #[test]
    fn test_collect_companions_invalid_input() {
        let input = quote! { struct };
        let result = collect_companions(input, "__prefix_");
        let result_str = tokens_to_string(&result);
        assert!(result_str.contains("compile_error"));
    }
}
