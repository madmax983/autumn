//! `#[static_get]` macro implementation.
//!
//! Generates both a regular route companion (`__autumn_route_info_{name}`)
//! AND a static metadata companion (`__autumn_static_meta_{name}`), marking
//! the handler for static pre-rendering at build time.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemFn, LitStr};

/// Core implementation for the `#[static_get("/path")]` attribute macro.
///
/// Emits:
/// 1. The original `async fn` unchanged.
/// 2. `__autumn_route_info_{name}()` returning `::autumn_web::route::Route`
///    (identical to what `#[get]` produces).
/// 3. `__autumn_static_meta_{name}()` returning
///    `::autumn_web::static_gen::StaticRouteMeta`.
pub fn static_get_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    // ── Parse path literal ──────────────────────────────────────────
    let path: LitStr = match syn::parse2(attr) {
        Ok(path) => path,
        Err(err) => return err.to_compile_error(),
    };

    // Validate: path not empty
    if path.value().is_empty() {
        return syn::Error::new(path.span(), "Route path must not be empty").to_compile_error();
    }

    // Validate: path starts with '/'
    if !path.value().starts_with('/') {
        let suggested = format!("/{}", path.value());
        return syn::Error::new(
            path.span(),
            format!("Route path must start with '/'. Did you mean \"{suggested}\"?"),
        )
        .to_compile_error();
    }

    // Validate: no path parameters (Phase 1 restriction)
    if path.value().contains('{') {
        return syn::Error::new(
            path.span(),
            "static_get does not support path parameters yet. Use #[get] for parameterized routes.",
        )
        .to_compile_error();
    }

    // ── Parse the annotated function ────────────────────────────────
    let input_fn: ItemFn = match syn::parse2(item.clone()) {
        Ok(f) => f,
        Err(_) => {
            return syn::Error::new_spanned(item, "static_get can only be applied to functions")
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

    // ── Code generation ─────────────────────────────────────────────
    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let static_meta_name = format_ident!("__autumn_static_meta_{}", fn_name);
    let vis = &input_fn.vis;

    quote! {
        #input_fn

        #[doc(hidden)]
        #vis fn #route_info_name() -> ::autumn_web::route::Route {
            ::autumn_web::route::Route {
                method: ::autumn_web::reexports::http::Method::GET,
                path: #path,
                handler: ::autumn_web::reexports::axum::routing::get(#fn_name),
                name: ::core::stringify!(#fn_name),
            }
        }

        #[doc(hidden)]
        #vis fn #static_meta_name() -> ::autumn_web::static_gen::StaticRouteMeta {
            ::autumn_web::static_gen::StaticRouteMeta {
                path: #path,
                name: ::core::stringify!(#fn_name),
                revalidate: ::core::option::Option::None,
            }
        }
    }
}
