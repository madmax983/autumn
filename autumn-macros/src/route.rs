//! Route macro implementation.
//!
//! Generates a companion `__autumn_route_info_{name}()` function for each
//! annotated handler, pairing the HTTP method and path with an Axum
//! `MethodRouter`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ReturnType, Type};

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

    let primitive_wrapper = if should_stringify_primitive_output(&input_fn.sig.output) {
        let wrapper_name = format_ident!("__autumn_primitive_handler_{}", fn_name);
        let mut wrapper_inputs = Vec::new();
        let mut call_args = Vec::new();

        for (idx, arg) in input_fn.sig.inputs.iter().enumerate() {
            match arg {
                FnArg::Typed(pat_type) => {
                    let arg_name = format_ident!("__autumn_arg_{idx}");
                    let ty = &pat_type.ty;
                    wrapper_inputs.push(quote! { #arg_name: #ty });
                    call_args.push(quote! { #arg_name });
                }
                FnArg::Receiver(receiver) => {
                    return syn::Error::new_spanned(
                        receiver,
                        "Autumn route handlers cannot take a self receiver",
                    )
                    .to_compile_error();
                }
            }
        }

        Some(quote! {
            #[doc(hidden)]
            async fn #wrapper_name(#(#wrapper_inputs),*) -> ::std::string::String {
                #fn_name(#(#call_args),*).await.to_string()
            }
        })
    } else {
        None
    };

    let handler_name = primitive_wrapper
        .as_ref()
        .map(|_| format_ident!("__autumn_primitive_handler_{}", fn_name))
        .unwrap_or_else(|| fn_name.clone());

    // Build the handler expression, chaining .layer() for each interceptor.
    // Interceptors are applied in reverse attribute order so that the first
    // #[intercept(...)] listed is the outermost layer (runs first).
    let mut handler_expr: TokenStream =
        quote! { ::autumn_web::reexports::axum::routing::#routing_fn(#handler_name) };

    for interceptor in interceptors.iter().rev() {
        // Explicit error type annotation avoids inference ambiguity when
        // multiple .layer() calls are chained on MethodRouter.
        handler_expr = quote! {
            ::autumn_web::reexports::axum::routing::MethodRouter::<
                ::autumn_web::AppState, ::core::convert::Infallible
            >::layer(#handler_expr, #interceptor)
        };
    }

    quote! {
        // ECHO-001: We want to apply #[axum::debug_handler] but without forcing the user
        // to import axum manually. However, the path resolution in Axum macros makes this impossible
        // natively. Custom compile errors handle the type checks.
        #input_fn
        #primitive_wrapper

        #[doc(hidden)]
        #vis fn #route_info_name() -> ::autumn_web::Route {
            ::autumn_web::Route {
                method: ::autumn_web::reexports::http::Method::#method_const,
                path: #path,
                handler: #handler_expr,
                name: ::core::stringify!(#fn_name),
            }
        }
    }
}

fn should_stringify_primitive_output(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };

    let Type::Path(path) = ty.as_ref() else {
        return false;
    };

    if path.qself.is_some() || path.path.segments.len() != 1 {
        return false;
    }

    let ident = path.path.segments[0].ident.to_string();
    matches!(
        ident.as_str(),
        "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}
