//! `#[step_up]` proc macro implementation.
//!
//! Generates a step-up authentication guard that runs before the handler
//! body. Injects hidden extractors and prepends a call to the runtime
//! check function.
//!
//! ## Forms
//!
//! - `#[step_up]` -- require fresh auth with the default max-age (5 minutes)
//! - `#[step_up(max_age = "5m")]` -- require fresh auth within 5 minutes
//! - `#[step_up(max_age = "1h")]` -- require fresh auth within 1 hour

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_quote};

use crate::param_helpers::has_input_named;

/// Parse the `#[step_up(max_age = "…")]` attribute arguments.
///
/// Returns `Some(seconds)` when `max_age` is specified, `None` for bare
/// `#[step_up]`.
fn parse_step_up_args(attr: TokenStream) -> syn::Result<Option<u64>> {
    if attr.is_empty() {
        return Ok(None);
    }

    let meta: syn::MetaNameValue = syn::parse2(attr)?;
    let key = meta
        .path
        .get_ident()
        .map(std::string::ToString::to_string);
    if key.as_deref() != Some("max_age") {
        return Err(syn::Error::new_spanned(
            &meta.path,
            "#[step_up] only accepts a `max_age` argument (e.g. #[step_up(max_age = \"5m\")])",
        ));
    }

    let value_str: LitStr = match &meta.value {
        syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
            syn::Lit::Str(s) => s.clone(),
            _ => {
                return Err(syn::Error::new_spanned(
                    &meta.value,
                    "max_age must be a string literal, e.g. \"5m\"",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &meta.value,
                "max_age must be a string literal, e.g. \"5m\"",
            ))
        }
    };

    let secs = parse_max_age_str_at_compile_time(&value_str)
        .map_err(|msg| syn::Error::new_spanned(&value_str, msg))?;
    Ok(Some(secs))
}

/// Parse a duration string at macro-expansion time.
fn parse_max_age_str_at_compile_time(lit: &LitStr) -> Result<u64, String> {
    let s = lit.value();
    if let Some(mins) = s.strip_suffix('m') {
        return mins
            .parse::<u64>()
            .map(|m| m * 60)
            .map_err(|_| format!("invalid max_age: '{s}' (expected e.g. \"5m\")"));
    }
    if let Some(hours) = s.strip_suffix('h') {
        return hours
            .parse::<u64>()
            .map(|h| h * 3600)
            .map_err(|_| format!("invalid max_age: '{s}' (expected e.g. \"1h\")"));
    }
    if let Some(secs) = s.strip_suffix('s') {
        return secs
            .parse::<u64>()
            .map_err(|_| format!("invalid max_age: '{s}' (expected e.g. \"30s\")"));
    }
    s.parse::<u64>()
        .map_err(|_| format!("invalid max_age: '{s}' (expected seconds or e.g. \"5m\")"))
}

/// Expand the `#[step_up]` / `#[step_up(max_age = "Nm")]` attribute.
pub fn step_up_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let max_age_opt = match parse_step_up_args(attr) {
        Ok(v) => v,
        Err(err) => return err.to_compile_error(),
    };

    let mut input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "#[step_up] can only be applied to async functions",
        )
        .to_compile_error();
    }

    // Emit a `Some(N_u64)` or `None` token at the call site.
    let max_age_tokens = match max_age_opt {
        Some(n) => {
            let lit = proc_macro2::Literal::u64_suffixed(n);
            quote! { ::core::option::Option::Some(#lit) }
        }
        None => quote! { ::core::option::Option::None },
    };

    // The resolved max-age used in the WWW-Authenticate header. At runtime this
    // is either the route-level value or the framework default (300).
    let check_call = quote! {
        const __AUTUMN_STEP_UP_MAX_AGE: ::core::option::Option<u64> = #max_age_tokens;
        if let ::core::result::Result::Err(__autumn_step_up_error) =
            ::autumn_web::step_up::__check_step_up_with_config(
                &__autumn_session,
                &__autumn_state,
                __AUTUMN_STEP_UP_MAX_AGE,
            ).await
        {
            let __max_age_secs: u64 = __AUTUMN_STEP_UP_MAX_AGE
                .unwrap_or(::autumn_web::step_up::DEFAULT_MAX_AGE_SECS);

            // Detect API clients by inspecting the Accept header.
            let __wants_json: bool = __autumn_step_up_headers
                .get(::autumn_web::reexports::axum::http::header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("application/json"))
                .unwrap_or(false);

            if __wants_json {
                return ::autumn_web::step_up::__step_up_json_response(__max_age_secs);
            } else {
                let __return_to: ::std::string::String =
                    ::autumn_web::step_up::encode_return_to(
                        __autumn_step_up_uri.path()
                    );
                return ::autumn_web::reexports::axum::response::IntoResponse::into_response(
                    ::autumn_web::reexports::axum::response::Redirect::to(
                        &::std::format!("/reauth?return_to={__return_to}")
                    )
                );
            }
        }
    };

    let original_body = &input_fn.block;
    let original_response = match &input_fn.sig.output {
        syn::ReturnType::Default => quote! {
            let __autumn_inner: () = (async move #original_body).await;
            ::autumn_web::reexports::axum::response::IntoResponse::into_response(__autumn_inner)
        },
        syn::ReturnType::Type(_, ty) if matches!(ty.as_ref(), syn::Type::ImplTrait(_)) => quote! {
            ::autumn_web::reexports::axum::response::IntoResponse::into_response(
                (async move #original_body).await
            )
        },
        syn::ReturnType::Type(_, ty) => quote! {
            let __autumn_inner: #ty = (async move #original_body).await;
            ::autumn_web::reexports::axum::response::IntoResponse::into_response(__autumn_inner)
        },
    };

    // Inject hidden extractors; guard against duplication when #[step_up]
    // is stacked with #[secured] or #[authorize].
    if !has_input_named(&input_fn, "__autumn_state") {
        let state_param: syn::FnArg = parse_quote! {
            ::autumn_web::reexports::axum::extract::State(__autumn_state):
                ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>
        };
        input_fn.sig.inputs.insert(0, state_param);
    }
    if !has_input_named(&input_fn, "__autumn_session") {
        let session_param: syn::FnArg = parse_quote! {
            __autumn_session: ::autumn_web::session::Session
        };
        input_fn.sig.inputs.insert(0, session_param);
    }
    if !has_input_named(&input_fn, "__autumn_step_up_headers") {
        let headers_param: syn::FnArg = parse_quote! {
            __autumn_step_up_headers: ::autumn_web::reexports::axum::http::HeaderMap
        };
        input_fn.sig.inputs.insert(0, headers_param);
    }
    if !has_input_named(&input_fn, "__autumn_step_up_uri") {
        let uri_param: syn::FnArg = parse_quote! {
            __autumn_step_up_uri: ::autumn_web::reexports::axum::http::Uri
        };
        input_fn.sig.inputs.insert(0, uri_param);
    }

    input_fn
        .attrs
        .push(parse_quote!(#[allow(clippy::too_many_arguments)]));
    input_fn.sig.output = parse_quote! {
        -> ::autumn_web::reexports::axum::response::Response
    };

    input_fn.block = syn::parse_quote! {
        {
            #check_call
            #original_response
        }
    };

    quote! { #input_fn }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::step_up_macro;

    #[test]
    fn step_up_bare_generates_check_call() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn delete_account() -> &'static str {
                    "deleted"
                }
            },
        )
        .to_string();
        assert!(
            generated.contains("__check_step_up_with_config"),
            "bare #[step_up] should generate a step-up check:\n{generated}"
        );
    }

    #[test]
    fn step_up_with_max_age_minutes_emits_seconds() {
        let generated = step_up_macro(
            quote! { max_age = "5m" },
            quote! {
                async fn delete_account() -> &'static str {
                    "deleted"
                }
            },
        )
        .to_string();
        assert!(
            generated.contains("__check_step_up_with_config"),
            "should contain step-up check:\n{generated}"
        );
        assert!(
            generated.contains("300u64"),
            "5m should expand to 300u64:\n{generated}"
        );
    }

    #[test]
    fn step_up_with_max_age_hours() {
        let generated = step_up_macro(
            quote! { max_age = "1h" },
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("3600u64"),
            "1h should expand to 3600u64:\n{generated}"
        );
    }

    #[test]
    fn step_up_with_max_age_seconds() {
        let generated = step_up_macro(
            quote! { max_age = "30s" },
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("30u64"),
            "30s should expand to 30u64:\n{generated}"
        );
    }

    #[test]
    fn step_up_injects_session_parameter() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("__autumn_session"),
            "should inject session parameter:\n{generated}"
        );
    }

    #[test]
    fn step_up_injects_state_parameter() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("__autumn_state"),
            "should inject state parameter:\n{generated}"
        );
    }

    #[test]
    fn step_up_injects_headers_parameter() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("__autumn_step_up_headers"),
            "should inject headers parameter:\n{generated}"
        );
    }

    #[test]
    fn step_up_injects_uri_parameter() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("__autumn_step_up_uri"),
            "should inject URI parameter:\n{generated}"
        );
    }

    #[test]
    fn step_up_rejects_sync_functions() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                fn sync_handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("compile_error"),
            "should emit compile_error for non-async functions:\n{generated}"
        );
    }

    #[test]
    fn step_up_rejects_unknown_attribute_key() {
        let generated = step_up_macro(
            quote! { unknown_arg = "value" },
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        assert!(
            generated.contains("compile_error"),
            "should emit compile_error for unknown attribute key:\n{generated}"
        );
    }

    #[test]
    fn step_up_generates_redirect_for_html_client() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        // Should redirect to /reauth?return_to=… for non-JSON clients
        assert!(
            generated.contains("/reauth"),
            "should redirect to /reauth for HTML clients:\n{generated}"
        );
    }

    #[test]
    fn step_up_generates_json_response_branch() {
        let generated = step_up_macro(
            quote! {},
            quote! {
                async fn handler() -> &'static str { "ok" }
            },
        )
        .to_string();
        // Should call __step_up_json_response for JSON clients
        assert!(
            generated.contains("__step_up_json_response"),
            "should call JSON response helper for API clients:\n{generated}"
        );
    }

    #[test]
    fn step_up_does_not_duplicate_session_when_stacked_with_secured() {
        // Simulate what happens when both #[secured] and #[step_up] are applied:
        // both macros try to inject __autumn_session. The has_input_named guard
        // should prevent duplicates.
        let after_secured = step_up_macro(
            quote! {},
            // Function already has __autumn_session (as if #[secured] ran first)
            quote! {
                async fn handler(
                    __autumn_session: ::autumn_web::session::Session,
                    __autumn_state: ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>,
                ) -> &'static str { "ok" }
            },
        )
        .to_string();
        // Count occurrences of "__autumn_session" — should appear multiple times
        // in generated code (parameter, call site) but the *parameter declaration*
        // should only appear once.
        let session_decl_count = after_secured
            .matches("__autumn_session : :: autumn_web :: session :: Session")
            .count();
        assert_eq!(
            session_decl_count, 1,
            "should not duplicate __autumn_session parameter:\n{after_secured}"
        );
    }
}
