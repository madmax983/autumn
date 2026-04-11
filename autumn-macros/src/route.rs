//! Route macro implementation.
//!
//! Generates a companion `__autumn_route_info_{name}()` function for each
//! annotated handler, pairing the HTTP method and path with an Axum
//! `MethodRouter`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parse;

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
    let path = match parse::parse_route_path(attr) {
        Ok(p) => p,
        Err(err) => return err,
    };

    let mut input_fn = match parse::parse_async_handler(item) {
        Ok(f) => f,
        Err(err) => return err,
    };

    // Extract #[intercept(LayerType)] attributes from the handler.
    let interceptors = parse::extract_interceptors(&mut input_fn.attrs);

    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let vis = &input_fn.vis;

    let method_const = format_ident!("{}", http_method); // e.g., GET
    let routing_fn = format_ident!("{}", axum_fn); // e.g., get

    // Build the handler expression, chaining .layer() for each interceptor.
    // Interceptors are applied in reverse attribute order so that the first
    // #[intercept(...)] listed is the outermost layer (runs first).
    let mut handler_expr: TokenStream =
        quote! { ::autumn_web::reexports::axum::routing::#routing_fn(#fn_name) };

    for interceptor in interceptors.iter().rev() {
        // Explicit error type annotation avoids inference ambiguity when
        // multiple .layer() calls are chained on MethodRouter.
        handler_expr = quote! {
            ::autumn_web::reexports::axum::routing::MethodRouter::<
                ::autumn_web::AppState, ::core::convert::Infallible
            >::layer(#handler_expr, #interceptor)
        };
    }

    // Note: we intentionally do NOT apply #[axum::debug_handler] here.
    // That macro generates code with `::axum::` paths, which don't resolve
    // when the user only depends on `autumn-web` (axum is a transitive dep).
    // Custom compile_error! diagnostics (S-007) provide error guidance instead.

    quote! {
        // ECHO-001: We want to apply #[axum::debug_handler] but without forcing the user
        // to import axum manually. However, the path resolution in Axum macros makes this impossible
        // natively. Custom compile errors handle the type checks.
        #input_fn

        #[doc(hidden)]
        #vis fn #route_info_name() -> ::autumn_web::route::Route {
            ::autumn_web::route::Route {
                method: ::autumn_web::reexports::http::Method::#method_const,
                path: #path,
                handler: #handler_expr,
                name: ::core::stringify!(#fn_name),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_route_macro() {
        let attr = quote! { "/test" };
        let item = quote! {
            async fn test_handler() -> String {
                "test".to_string()
            }
        };

        let result = route_macro("GET", "get", attr, item);
        let result_str = result.to_string();

        assert!(result_str.contains("__autumn_route_info_test_handler"));
        assert!(result_str.contains("Method :: GET"));
        assert!(result_str.contains("routing :: get (test_handler)"));
    }

    #[test]
    fn test_route_macro_with_intercept() {
        let attr = quote! { "/test" };
        let item = quote! {
            #[intercept(MyLayer)]
            async fn test_handler() -> String {
                "test".to_string()
            }
        };

        let result = route_macro("POST", "post", attr, item);
        let result_str = result.to_string();

        assert!(result_str.contains("__autumn_route_info_test_handler"));
        assert!(result_str.contains("Method :: POST"));
        assert!(result_str.contains("layer ("));
        assert!(result_str.contains("MyLayer"));
    }
}
