//! Route macro implementation.
//!
//! Generates a companion `__autumn_route_info_{name}()` function for each
//! annotated handler, pairing the HTTP method and path with an Axum
//! `MethodRouter`. The companion also carries an [`ApiDoc`] describing
//! the route for `OpenAPI` auto-generation.
//!
//! Additionally emits a `__autumn_path_{name}(params...)` companion that
//! returns a [`PathBuilder`](::autumn_web::PathBuilder) for use with the
//! [`paths![]`](::autumn_web::paths) collection macro.
//!
//! [`ApiDoc`]: ../../autumn_web/openapi/struct.ApiDoc.html

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, GenericArgument, PathArguments, ReturnType, Type};

use crate::api_doc;
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

    // Extract #[api_doc(...)] overrides before emitting the function, so
    // the attribute doesn't leak onto the emitted fn definition and
    // trigger an "unknown attribute" error.
    let api_doc_attr = match api_doc::extract(&mut input_fn.attrs) {
        Ok(v) => v,
        Err(err) => return err,
    };

    // Extract optional `name = "..."` override for the path helper.
    let helper_name_override = extract_name_override(&mut input_fn.attrs);

    let fn_name = &input_fn.sig.ident;
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);
    let path_helper_base = helper_name_override
        .as_deref()
        .unwrap_or(&fn_name.to_string())
        .to_owned();
    let path_helper_name = format_ident!("__autumn_path_{}", path_helper_base);
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

    // ── Path helper companion ────────────────────────────────────
    let path_helper_fn = emit_path_helper(
        &path_helper_name,
        &path.value(),
        &path_params,
        &input_fn,
    );

    quote! {
        // ECHO-001: We want to apply #[axum::debug_handler] but without forcing the user
        // to import axum manually. However, the path resolution in Axum macros makes this impossible
        // natively. Custom compile errors handle the type checks.
        #input_fn
        #primitive_wrapper
        #path_helper_fn

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
                repository: ::core::option::Option::None,
            }
        }
    }
}

/// Emit the `__autumn_path_{name}(params...) -> PathBuilder` companion.
///
/// Uses `impl Display` for each param in the Green Phase. The Refactor Phase
/// replaces these with extracted types from `Path<T>` extractors.
fn emit_path_helper(
    helper_name: &proc_macro2::Ident,
    path_str: &str,
    path_params: &[String],
    input_fn: &syn::ItemFn,
) -> TokenStream {
    let format_str = build_path_format_string(path_str);
    let param_types = extract_path_param_types(path_params, input_fn);

    let helper_params: Vec<TokenStream> = path_params
        .iter()
        .zip(param_types.iter())
        .map(|(name, ty)| {
            let ident = format_ident!("{}", name);
            quote! { #ident: #ty }
        })
        .collect();

    let param_idents: Vec<proc_macro2::Ident> = path_params
        .iter()
        .map(|name| format_ident!("{}", name))
        .collect();

    if path_params.is_empty() {
        quote! {
            #[doc(hidden)]
            pub fn #helper_name() -> ::autumn_web::PathBuilder {
                ::autumn_web::PathBuilder::new(::std::string::String::from(#path_str))
            }
        }
    } else {
        quote! {
            #[doc(hidden)]
            pub fn #helper_name(#(#helper_params),*) -> ::autumn_web::PathBuilder {
                ::autumn_web::PathBuilder::new(::std::format!(#format_str, #(#param_idents),*))
            }
        }
    }
}

/// Build a `format!`-compatible string from a route path template.
///
/// Replaces each `{param}` or `{param:regex}` segment with `{}`.
fn build_path_format_string(path: &str) -> String {
    let mut fmt = String::with_capacity(path.len());
    let mut remaining = path;

    while !remaining.is_empty() {
        if let Some(start) = remaining.find('{') {
            fmt.push_str(&remaining[..start]);
            let after_brace = &remaining[start + 1..];
            if let Some(end_rel) = after_brace.find('}') {
                fmt.push_str("{}");
                remaining = &after_brace[end_rel + 1..];
            } else {
                // Unclosed brace — include literally (parse validates paths,
                // but be defensive here).
                fmt.push_str(&remaining[start..]);
                break;
            }
        } else {
            fmt.push_str(remaining);
            break;
        }
    }

    fmt
}

/// Extract the inner `T` from a `Path<T>` type annotation, if present.
fn inner_of_path_extractor(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    if type_path.qself.is_some() {
        return None;
    }
    let last = type_path.path.segments.last()?;
    if last.ident != "Path" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    if let GenericArgument::Type(inner) = args.args.first()? {
        Some(inner)
    } else {
        None
    }
}

/// Determine the parameter types for the path helper.
///
/// Scans the handler's parameter list for the first `Path<T>` extractor
/// and maps T's elements (or T itself for a single param) onto the ordered
/// list of `path_params` extracted from the route template.
///
/// Falls back to `impl ::std::fmt::Display` for any param not covered by a
/// typed extractor — this handles routes that use `String` path params or
/// custom `FromStr`/`Display` types.
fn extract_path_param_types(path_params: &[String], input_fn: &syn::ItemFn) -> Vec<TokenStream> {
    if path_params.is_empty() {
        return Vec::new();
    }

    for arg in &input_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let Some(inner) = inner_of_path_extractor(&pat_type.ty) else {
            continue;
        };

        // Found a Path<T> extractor.
        match inner {
            Type::Tuple(tuple) => {
                // Path<(T1, T2, ...)> — zip element types with param names.
                let types: Vec<TokenStream> = tuple.elems.iter().map(|t| quote! { #t }).collect();
                // If the tuple has fewer elements than params, fall back for extras.
                let mut result = Vec::with_capacity(path_params.len());
                for i in 0..path_params.len() {
                    if let Some(ty) = types.get(i) {
                        result.push(ty.clone());
                    } else {
                        result.push(quote! { impl ::std::fmt::Display });
                    }
                }
                return result;
            }
            single_ty => {
                // Path<T> — single param.
                let ty_tokens = quote! { #single_ty };
                let mut result = Vec::with_capacity(path_params.len());
                result.push(ty_tokens);
                for _ in 1..path_params.len() {
                    result.push(quote! { impl ::std::fmt::Display });
                }
                return result;
            }
        }
    }

    // No Path<T> extractor found — use Display for all params.
    path_params
        .iter()
        .map(|_| quote! { impl ::std::fmt::Display })
        .collect()
}

/// Strip an optional `#[name = "custom"]` attribute from the handler,
/// returning the custom name string if present.
fn extract_name_override(attrs: &mut Vec<syn::Attribute>) -> Option<String> {
    let mut result = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("name") {
            return true;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                result = Some(s.value());
            }
        }
        false
    });
    result
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

// ── Unit tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_path_format_string_no_params() {
        assert_eq!(build_path_format_string("/posts"), "/posts");
    }

    #[test]
    fn build_path_format_string_single_param() {
        assert_eq!(build_path_format_string("/posts/{id}"), "/posts/{}");
    }

    #[test]
    fn build_path_format_string_two_params() {
        assert_eq!(
            build_path_format_string("/posts/{post_id}/comments/{comment_id}"),
            "/posts/{}/comments/{}"
        );
    }

    #[test]
    fn build_path_format_string_regex_param() {
        assert_eq!(
            build_path_format_string("/items/{id:[0-9]+}"),
            "/items/{}"
        );
    }

    #[test]
    fn build_path_format_string_trailing_slash() {
        assert_eq!(build_path_format_string("/search/"), "/search/");
    }

    #[test]
    fn build_path_format_string_root() {
        assert_eq!(build_path_format_string("/"), "/");
    }

}
