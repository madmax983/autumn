//! Route macro implementation.
//!
//! Generates a companion `__autumn_route_info_{name}()` function for each
//! annotated handler, pairing the HTTP method and path with an Axum
//! `MethodRouter`. The companion also carries an [`ApiDoc`] describing
//! the route for `OpenAPI` auto-generation.
//!
//! [`ApiDoc`]: ../../autumn_web/openapi/struct.ApiDoc.html

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ReturnType, Type};

use crate::api_doc;
use crate::parse;

/// Core implementation shared by all route macros (`#[get]`, `#[post]`, etc.).
///
/// `http_method` is the uppercase method name (e.g., `"GET"`).
#[allow(clippy::too_many_lines)]
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

    // Extract #[api_doc(...)] overrides before emitting the function, so
    // the attribute doesn't leak onto the emitted fn definition and
    // trigger an "unknown attribute" error.
    let api_doc_attr = match api_doc::extract(&mut input_fn.attrs) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let vis = &input_fn.vis;

    let method_const = format_ident!("{}", http_method); // e.g., GET
    let routing_fn = format_ident!("{}", axum_fn); // e.g., get

    let primitive_wrapper = if let Some(prim_type) = check_primitive_output(&input_fn.sig.output) {
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

        let body = match prim_type {
            PrimitiveType::Bare => quote! {
                #fn_name(#(#call_args),*).await.to_string()
            },
            PrimitiveType::Result => quote! {
                match #fn_name(#(#call_args),*).await {
                    ::std::result::Result::Ok(val) => ::std::result::Result::Ok(val.to_string()),
                    ::std::result::Result::Err(err) => ::std::result::Result::Err(err),
                }
            },
        };

        let ret_ty = match prim_type {
            PrimitiveType::Bare => quote! { -> ::std::string::String },
            PrimitiveType::Result => {
                let ReturnType::Type(_, ty) = &input_fn.sig.output else {
                    unreachable!()
                };
                let Type::Path(path) = ty.as_ref() else {
                    unreachable!()
                };
                let err_ty = if let syn::PathArguments::AngleBracketed(args) =
                    &path.path.segments.last().unwrap().arguments
                {
                    if args.args.len() == 2 {
                        let err_arg = &args.args[1];
                        quote! { #err_arg }
                    } else {
                        quote! { _ }
                    }
                } else {
                    quote! { _ }
                };
                quote! { -> ::std::result::Result<::std::string::String, #err_ty> }
            }
        };

        Some(quote! {
            #[doc(hidden)]
            async fn #wrapper_name(#(#wrapper_inputs),*) #ret_ty {
                #body
            }
        })
    } else {
        None
    };

    let handler_name = primitive_wrapper.as_ref().map_or_else(
        || fn_name.clone(),
        |_| format_ident!("__autumn_primitive_handler_{}", fn_name),
    );

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

    // ── OpenAPI metadata ────────────────────────────────────────
    let path_params = api_doc::extract_path_params(&path.value());
    let path_params_tokens = api_doc::emit_path_param_slice(&path_params);
    let request_body = api_doc::schema_option(api_doc::infer_request_body(&input_fn));
    let response_body = api_doc::schema_option(api_doc::infer_response_body(&input_fn));
    let api_doc_fields = api_doc_attr.emit_ident_fields(fn_name);
    let http_method_lit = syn::LitStr::new(http_method, proc_macro2::Span::call_site());

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
                api_doc: ::autumn_web::openapi::ApiDoc {
                    method: #http_method_lit,
                    path: #path,
                    path_params: #path_params_tokens,
                    request_body: #request_body,
                    response: #response_body,
                    register_schemas: ::core::option::Option::None,
                    #api_doc_fields
                },
            }
        }
    }
}

enum PrimitiveType {
    Bare,
    Result,
}

fn check_primitive_output(output: &ReturnType) -> Option<PrimitiveType> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };

    let Type::Path(path) = ty.as_ref() else {
        return None;
    };

    if path.qself.is_some() || path.path.segments.is_empty() {
        return None;
    }

    let ident = path.path.segments.last().unwrap().ident.to_string();
    if ident == "Result" {
        // Check if the first generic argument is a primitive
        if let syn::PathArguments::AngleBracketed(args) =
            &path.path.segments.last().unwrap().arguments
        {
            if let Some(syn::GenericArgument::Type(Type::Path(inner_path))) = args.args.first() {
                if inner_path.qself.is_none() && inner_path.path.segments.len() == 1 {
                    let inner_ident = inner_path.path.segments[0].ident.to_string();
                    if is_primitive(&inner_ident) {
                        return Some(PrimitiveType::Result);
                    }
                }
            }
        }
        return None;
    }

    if path.path.segments.len() == 1 && is_primitive(&ident) {
        return Some(PrimitiveType::Bare);
    }

    None
}

fn is_primitive(ident: &str) -> bool {
    matches!(
        ident,
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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_primitive_check() {
        let bare: ReturnType = parse_quote! { -> i32 };
        assert!(matches!(
            check_primitive_output(&bare),
            Some(PrimitiveType::Bare)
        ));

        let res: ReturnType = parse_quote! { -> Result<i32, String> };
        assert!(matches!(
            check_primitive_output(&res),
            Some(PrimitiveType::Result)
        ));

        let complex: ReturnType = parse_quote! { -> Vec<i32> };
        assert!(matches!(check_primitive_output(&complex), None));

        let res_complex: ReturnType = parse_quote! { -> Result<Vec<i32>, String> };
        assert!(matches!(check_primitive_output(&res_complex), None));

        let no_ret: ReturnType = parse_quote! {};
        assert!(matches!(check_primitive_output(&no_ret), None));
    }
}
