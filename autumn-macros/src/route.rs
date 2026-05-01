//! Route macro implementation.
//!
//! Generates two companion functions for each annotated handler:
//!
//! 1. `__autumn_route_info_{name}()` — returns a `Route` (existing behaviour).
//! 2. `__autumn_path_{helper_name}(params…) -> String` — typed path helper
//!    that accepts one `impl Display` argument per `{param}` segment in the URL
//!    and returns the formatted absolute path string.
//!
//! The `helper_name` defaults to the handler function name but can be
//! overridden with the `name = "custom_name"` route attribute argument.
//!
//! [`ApiDoc`]: ../../autumn_web/openapi/struct.ApiDoc.html

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{FnArg, LitStr, ReturnType, Type};

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
    let route_args = match parse::parse_route_attr(attr) {
        Ok(a) => a,
        Err(err) => return err,
    };
    let path = route_args.path.clone();

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

    // Determine the path-helper name: use `name = "..."` override when set,
    // otherwise default to the handler function name.
    let helper_ident = route_args.helper_ident(fn_name);
    let path_helper_name = format_ident!("__autumn_path_{}", helper_ident);

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
    let http_method_lit = LitStr::new(http_method, Span::call_site());

    // ── Path helper ─────────────────────────────────────────────
    let path_helper = emit_path_helper(vis, &path_helper_name, &path, &path_params);

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
                repository: ::core::option::Option::None,
            }
        }

        #path_helper
    }
}

/// Emit the typed path helper function.
///
/// For `/posts/{id}/comments/{comment_id}` this emits:
/// ```ignore
/// pub fn __autumn_path_handler(id: impl Display, comment_id: impl Display) -> String {
///     format!("/posts/{id}/comments/{comment_id}")
/// }
/// ```
///
/// Regex-constrained params like `{id:[0-9]+}` are normalised to `{id}` in the
/// format string so `format!` can use the named-capture syntax.
fn emit_path_helper(
    vis: &syn::Visibility,
    helper_name: &proc_macro2::Ident,
    path: &LitStr,
    params: &[String],
) -> TokenStream {
    // Build parameter list: one `name: impl Display` per path param.
    // Sanitize param names: strip the `*` catch-all prefix and replace `-`
    // with `_` so the result is a valid Rust identifier in both the
    // function signature and the `format!` named-argument position.
    let param_idents: Vec<proc_macro2::Ident> = params
        .iter()
        .map(|p| {
            let sanitized = p.trim_start_matches('*').replace('-', "_");
            format_ident!("{}", sanitized)
        })
        .collect();

    // Normalise the path string: replace `{param:regex}` → `{param}` so the
    // format string uses named-argument syntax that Rust's `format!` understands.
    let format_str = normalise_path_for_format(&path.value());
    let format_lit = LitStr::new(&format_str, path.span());

    quote! {
        #[doc(hidden)]
        #vis fn #helper_name(#(#param_idents: impl ::std::fmt::Display),*) -> ::std::string::String {
            format!(#format_lit)
        }
    }
}

/// Replace `{param:regex}` with `{param}` so the path can be used as a
/// `format!` string with named-argument capture syntax (stabilised in Rust 1.58).
///
/// Also normalises catch-all and hyphenated params to valid Rust identifiers:
/// - `{*rest}` → `{rest}` (strip catch-all `*` prefix)
/// - `{param-name}` → `{param_name}` (replace `-` with `_`)
fn normalise_path_for_format(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            result.push('{');
            let mut param = String::new();
            // Track depth so quantifiers like `{id:[0-9]{1,3}}` don't end
            // the capture at the first inner `}`.
            let mut depth: u32 = 1;
            for inner in chars.by_ref() {
                match inner {
                    '{' => {
                        depth += 1;
                        param.push(inner);
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        param.push(inner);
                    }
                    _ => param.push(inner),
                }
            }
            // Strip `:regex` suffix (everything after the first `:`), then
            // strip `*` catch-all prefix, then replace `-` → `_`.
            let name = param.split(':').next().unwrap_or(&param);
            let name = name.trim_start_matches('*').replace('-', "_");
            result.push_str(&name);
            result.push('}');
        } else {
            result.push(c);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::normalise_path_for_format;

    #[test]
    fn normalise_plain_params() {
        assert_eq!(normalise_path_for_format("/posts/{id}"), "/posts/{id}");
    }

    #[test]
    fn normalise_regex_constrained_params() {
        assert_eq!(
            normalise_path_for_format("/users/{id:[0-9]+}"),
            "/users/{id}"
        );
    }

    #[test]
    fn normalise_multiple_params() {
        assert_eq!(
            normalise_path_for_format("/posts/{year}/{slug}"),
            "/posts/{year}/{slug}"
        );
    }

    #[test]
    fn normalise_static_path() {
        assert_eq!(normalise_path_for_format("/hello"), "/hello");
    }

    #[test]
    fn normalise_catch_all_param() {
        assert_eq!(normalise_path_for_format("/files/{*path}"), "/files/{path}");
    }

    #[test]
    fn normalise_hyphenated_param() {
        assert_eq!(
            normalise_path_for_format("/items/{item-id}"),
            "/items/{item_id}"
        );
    }

    #[test]
    fn normalise_regex_with_quantifier_braces() {
        // Regex quantifiers like {1,3} must not end the outer capture early.
        assert_eq!(
            normalise_path_for_format("/users/{id:[0-9]{1,3}}"),
            "/users/{id}"
        );
    }
}
