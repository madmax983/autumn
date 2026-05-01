//! Shared parsing and validation helpers for route macros.

use proc_macro2::TokenStream;
use quote::format_ident;
use syn::{Attribute, Ident, ItemFn, LitStr, Token};

/// Parsed route macro attribute arguments.
///
/// Supports:
/// - `"/path"` — path only
/// - `"/path", name = "helper_name"` — path with custom helper name
pub struct RouteAttrArgs {
    pub path: LitStr,
    /// Override for the path-helper function name. When `None`, the helper
    /// name matches the handler function name.
    pub name_override: Option<LitStr>,
}

impl RouteAttrArgs {
    /// Return the helper name as an `Ident`, using the override if set.
    /// `handler_name` is used as the fallback.
    pub fn helper_ident(&self, handler_name: &Ident) -> Ident {
        self.name_override.as_ref().map_or_else(
            || handler_name.clone(),
            |lit| format_ident!("{}", lit.value()),
        )
    }
}

impl syn::parse::Parse for RouteAttrArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;

        let name_override = if input.peek(Token![,]) {
            let _comma: Token![,] = input.parse()?;
            let key: Ident = input.parse()?;
            if key != "name" {
                return Err(syn::Error::new(
                    key.span(),
                    format!(
                        "unknown route attribute key `{key}`. \
                         Supported keys: `name`."
                    ),
                ));
            }
            let _eq: Token![=] = input.parse()?;
            Some(input.parse::<LitStr>()?)
        } else {
            None
        };

        Ok(Self {
            path,
            name_override,
        })
    }
}

/// Parse and validate a route attribute with optional `name = "..."` override.
///
/// Returns `Ok(args)` if valid, or a compile error `TokenStream` if not.
pub fn parse_route_attr(attr: TokenStream) -> Result<RouteAttrArgs, TokenStream> {
    let args: RouteAttrArgs = syn::parse2(attr).map_err(|err| err.to_compile_error())?;
    validate_path(&args.path)?;
    Ok(args)
}

/// Parse and validate a route path from macro attributes.
///
/// Returns `Ok(path)` if valid, or a compile error `TokenStream` if not.
/// Validates: non-empty, starts with '/'.
pub fn parse_route_path(attr: TokenStream) -> Result<LitStr, TokenStream> {
    let path: LitStr = syn::parse2(attr).map_err(|err| err.to_compile_error())?;
    validate_path(&path)?;
    Ok(path)
}

fn validate_path(path: &LitStr) -> Result<(), TokenStream> {
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

    Ok(())
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

/// Extract `#[intercept(LayerType)]` attributes from a function's attribute
/// list, removing them so they don't appear on the emitted function.
///
/// Returns the type paths in the order they appeared.
pub fn extract_interceptors(attrs: &mut Vec<Attribute>) -> Vec<syn::Path> {
    let mut interceptors = Vec::new();
    attrs.retain(|attr| {
        if attr.path().is_ident("intercept") {
            if let Ok(path) = attr.parse_args::<syn::Path>() {
                interceptors.push(path);
            }
            false // remove from the attribute list
        } else {
            true // keep
        }
    });
    interceptors
}
