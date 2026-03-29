//! `#[static_get]` macro implementation.
//!
//! Generates both a regular route companion (`__autumn_route_info_{name}`)
//! AND a static metadata companion (`__autumn_static_meta_{name}`), marking
//! the handler for static pre-rendering at build time.
//!
//! ## Supported forms
//!
//! - `#[static_get("/about")]` -- simple static route
//! - `#[static_get("/posts/{slug}", params = list_slugs)]` -- parameterized
//! - `#[static_get("/posts/{slug}", params = list_slugs, revalidate = 60)]` -- with ISR

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitInt, LitStr, Token};

/// Parsed attributes for `#[static_get("/path", params = fn, revalidate = N)]`.
struct StaticGetAttrs {
    path: LitStr,
    params_fn: Option<syn::Path>,
    revalidate: Option<u64>,
}

impl Parse for StaticGetAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;

        let mut params_fn = None;
        let mut revalidate = None;

        while input.peek(Token![,]) {
            let _comma: Token![,] = input.parse()?;

            // Allow trailing comma
            if input.is_empty() {
                break;
            }

            let key: Ident = input.parse()?;
            let _eq: Token![=] = input.parse()?;

            match key.to_string().as_str() {
                "params" => {
                    let path: syn::Path = input.parse()?;
                    params_fn = Some(path);
                }
                "revalidate" => {
                    let lit: LitInt = input.parse()?;
                    revalidate = Some(lit.base10_parse::<u64>()?);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("Unknown attribute `{other}`. Expected `params` or `revalidate`."),
                    ));
                }
            }
        }

        Ok(Self {
            path,
            params_fn,
            revalidate,
        })
    }
}

/// Core implementation for the `#[static_get("/path")]` attribute macro.
///
/// Emits:
/// 1. The original `async fn` unchanged.
/// 2. `__autumn_route_info_{name}()` returning `::autumn_web::route::Route`
///    (identical to what `#[get]` produces).
/// 3. `__autumn_static_meta_{name}()` returning
///    `::autumn_web::static_gen::StaticRouteMeta`.
pub fn static_get_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs: StaticGetAttrs = match syn::parse2(attr) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error(),
    };

    let path = &attrs.path;

    // Validate path
    if path.value().is_empty() {
        return syn::Error::new(path.span(), "Route path must not be empty").to_compile_error();
    }

    if !path.value().starts_with('/') {
        let suggested = format!("/{}", path.value());
        return syn::Error::new(
            path.span(),
            format!("Route path must start with '/'. Did you mean \"{suggested}\"?"),
        )
        .to_compile_error();
    }

    // Parameterized routes require a params function
    let has_params = path.value().contains('{');
    if has_params && attrs.params_fn.is_none() {
        return syn::Error::new(
            path.span(),
            "Parameterized static routes require a `params` function. \
             Example: #[static_get(\"/posts/{slug}\", params = list_slugs)]",
        )
        .to_compile_error();
    }

    // Non-parameterized routes should not have a params function
    if !has_params && attrs.params_fn.is_some() {
        return syn::Error::new(
            path.span(),
            "Static route has no path parameters but a `params` function was provided. \
             Either add path parameters or remove the `params` attribute.",
        )
        .to_compile_error();
    }

    let input_fn = match crate::parse::parse_async_handler(item) {
        Ok(f) => f,
        Err(err) => return err,
    };

    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let static_meta_name = format_ident!("__autumn_static_meta_{}", fn_name);
    let vis = &input_fn.vis;

    // Build the revalidate expression
    let revalidate_expr = attrs.revalidate.map_or_else(
        || quote! { ::core::option::Option::None },
        |secs| quote! { ::core::option::Option::Some(#secs) },
    );

    // Build the params_fn expression
    let params_fn_expr = attrs.params_fn.as_ref().map_or_else(
        || quote! { ::core::option::Option::None },
        |pf| {
            quote! {
                ::core::option::Option::Some(
                    |router: ::autumn_web::reexports::axum::Router|
                        -> ::core::pin::Pin<Box<dyn ::core::future::Future<
                            Output = Vec<::autumn_web::static_gen::StaticParams>
                        > + Send>> {
                        Box::pin(#pf(router))
                    }
                )
            }
        },
    );

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
                revalidate: #revalidate_expr,
                params_fn: #params_fn_expr,
            }
        }
    }
}
