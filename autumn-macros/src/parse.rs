//! Shared parsing and validation helpers for route macros.

use proc_macro2::TokenStream;
use syn::{ItemFn, LitStr};

/// Parse and validate a route path from macro attributes.
///
/// Returns `Ok(path)` if valid, or a compile error `TokenStream` if not.
/// Validates: non-empty, starts with '/'.
pub fn parse_route_path(attr: TokenStream) -> Result<LitStr, TokenStream> {
    let path: LitStr = syn::parse2(attr).map_err(|err| err.to_compile_error())?;

    if path.value().is_empty() {
        return Err(syn::Error::new(path.span(), "Route path must not be empty").to_compile_error());
    }

    if !path.value().starts_with('/') {
        let suggested = format!("/{}", path.value());
        return Err(syn::Error::new(
            path.span(),
            format!("Route path must start with '/'. Did you mean \"{suggested}\"?"),
        )
        .to_compile_error());
    }

    Ok(path)
}

/// Parse and validate an async handler function from macro input.
///
/// Returns `Ok(func)` if valid, or a compile error `TokenStream` if not.
/// Validates: is a function, is async.
pub fn parse_async_handler(item: TokenStream) -> Result<ItemFn, TokenStream> {
    let input_fn: ItemFn = syn::parse2(item.clone()).map_err(|_| {
        syn::Error::new_spanned(item, "route macros can only be applied to functions")
            .to_compile_error()
    })?;

    if input_fn.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "Autumn route handlers must be async functions",
        )
        .to_compile_error());
    }

    Ok(input_fn)
}
