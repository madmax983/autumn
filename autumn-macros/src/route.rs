//! Route macro implementation.
//!
//! Generates a companion `__autumn_route_info_{name}()` function for each
//! annotated handler, pairing the HTTP method and path with an Axum
//! `MethodRouter`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemFn, LitStr};

/// Core implementation shared by all route macros (`#[get]`, `#[post]`, etc.).
///
/// `http_method` is the uppercase method name (e.g., `"GET"`).
/// `axum_fn` is the lowercase axum routing function name (e.g., `"get"`).
pub fn route_macro(
    http_method: &str,
    axum_fn: &str,
    attr: TokenStream,
    item: TokenStream,
) -> TokenStream {
    // Parse the path string from the attribute: #[get("/hello")]
    let path: LitStr = match syn::parse2(attr) {
        Ok(path) => path,
        Err(err) => return err.to_compile_error(),
    };

    // Validate path is not empty
    if path.value().is_empty() {
        return syn::Error::new(path.span(), "Route path must not be empty").to_compile_error();
    }

    // Validate path starts with '/'
    if !path.value().starts_with('/') {
        let suggested = format!("/{}", path.value());
        return syn::Error::new(
            path.span(),
            format!("Route path must start with '/'. Did you mean \"{suggested}\"?"),
        )
        .to_compile_error();
    }

    // Parse the annotated item — must be a function.
    // Clone before parsing so we can use the original tokens for error spans.
    let input_fn: ItemFn = match syn::parse2(item.clone()) {
        Ok(f) => f,
        Err(_) => {
            return syn::Error::new_spanned(item, "route macros can only be applied to functions")
                .to_compile_error();
        }
    };

    // Validate: must be async
    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "Autumn route handlers must be async functions",
        )
        .to_compile_error();
    }

    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let vis = &input_fn.vis;

    let method_const = format_ident!("{}", http_method); // e.g., GET
    let routing_fn = format_ident!("{}", axum_fn); // e.g., get

    // Note: we intentionally do NOT apply #[axum::debug_handler] here.
    // That macro generates code with `::axum::` paths, which don't resolve
    // when the user only depends on `autumn` (axum is a transitive dep).
    // Custom compile_error! diagnostics (S-007) provide error guidance instead.

    quote! {
        #input_fn

        #[doc(hidden)]
        #vis fn #route_info_name() -> ::autumn::route::Route {
            ::autumn::route::Route {
                method: ::autumn::reexports::http::Method::#method_const,
                path: #path,
                handler: ::autumn::reexports::axum::routing::#routing_fn(#fn_name),
                name: ::core::stringify!(#fn_name),
            }
        }
    }
}
