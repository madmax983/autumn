//! `#[feature_flag("key")]` proc macro implementation.
//!
//! Gates the entire route on a feature flag. If the flag is disabled for the
//! current actor the handler responds with 404 Not Found (default) or
//! delegates to a custom fallback handler specified with
//! `#[feature_flag("key", fallback = my_fallback_handler)]`.
//!
//! ## Usage
//!
//! ```ignore
//! use autumn_web::prelude::*;
//!
//! #[get("/beta")]
//! #[feature_flag("beta_dashboard")]
//! async fn beta_dashboard() -> Markup {
//!     html! { h1 { "Beta!" } }
//! }
//! ```
//!
//! With a custom fallback:
//!
//! ```ignore
//! #[get("/experimental")]
//! #[feature_flag("experimental_feature", fallback = feature_disabled)]
//! async fn experimental() -> Markup { html! { "Experimental" } }
//!
//! async fn feature_disabled() -> impl IntoResponse {
//!     (StatusCode::NOT_FOUND, "Feature not available yet")
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ItemFn, LitStr, Token, parse::ParseStream, parse_quote};

struct FeatureFlagArgs {
    flag_key: String,
    fallback: Option<Expr>,
}

impl syn::parse::Parse for FeatureFlagArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let flag_key: LitStr = input.parse()?;
        let mut fallback = None;
        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            let ident: syn::Ident = input.parse()?;
            if ident != "fallback" {
                return Err(syn::Error::new_spanned(
                    ident,
                    "expected `fallback = <handler_fn>`",
                ));
            }
            let _: Token![=] = input.parse()?;
            fallback = Some(input.parse()?);
        }
        Ok(Self {
            flag_key: flag_key.value(),
            fallback,
        })
    }
}

pub fn feature_flag_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args: FeatureFlagArgs = match syn::parse2(attr) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error(),
    };

    let mut input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "#[feature_flag] can only be applied to async functions",
        )
        .to_compile_error();
    }

    let flag_key = &args.flag_key;

    let disabled_response = match &args.fallback {
        Some(fallback_fn) => quote! {
            return ::autumn_web::reexports::axum::response::IntoResponse::into_response(
                #fallback_fn().await
            );
        },
        None => quote! {
            return ::autumn_web::reexports::axum::response::IntoResponse::into_response(
                ::autumn_web::reexports::http::StatusCode::NOT_FOUND
            );
        },
    };

    let flag_check = quote! {
        let __autumn_flag_enabled = {
            let __flags_svc = __autumn_state.extension::<::autumn_web::feature_flags::FeatureFlagService>();
            match __flags_svc {
                Some(svc) => {
                    let actor_id: Option<String> = None;
                    svc.is_enabled(#flag_key, actor_id.as_deref())
                }
                None => false,
            }
        };
        if !__autumn_flag_enabled {
            #disabled_response
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

    use crate::param_helpers::has_input_named;
    if !has_input_named(&input_fn, "__autumn_state") {
        let state_param: syn::FnArg = parse_quote! {
            ::autumn_web::reexports::axum::extract::State(__autumn_state):
                ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>
        };
        input_fn.sig.inputs.insert(0, state_param);
    }

    input_fn
        .attrs
        .push(parse_quote!(#[allow(clippy::too_many_arguments)]));
    input_fn.sig.output = parse_quote! {
        -> ::autumn_web::reexports::axum::response::Response
    };

    input_fn.block = syn::parse_quote! {
        {
            #flag_check
            #original_response
        }
    };

    quote! { #input_fn }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::feature_flag_macro;

    #[test]
    fn feature_flag_generates_valid_code_for_simple_flag() {
        let result = feature_flag_macro(
            quote! { "my_flag" },
            quote! {
                async fn my_handler() -> &'static str {
                    "hello"
                }
            },
        );
        let code = result.to_string();
        // Must contain the flag key lookup
        assert!(code.contains("my_flag"), "flag key must appear in generated code: {code}");
        // Must inject the state parameter
        assert!(
            code.contains("__autumn_state"),
            "must inject state param: {code}"
        );
        // Must handle the disabled case
        assert!(
            code.contains("NOT_FOUND") || code.contains("404"),
            "must have 404 fallback: {code}"
        );
    }

    #[test]
    fn feature_flag_with_custom_fallback_uses_fallback_fn() {
        let result = feature_flag_macro(
            quote! { "my_flag", fallback = custom_fallback },
            quote! {
                async fn my_handler() -> &'static str {
                    "hello"
                }
            },
        );
        let code = result.to_string();
        assert!(
            code.contains("custom_fallback"),
            "custom fallback fn must appear in generated code: {code}"
        );
        assert!(
            !code.contains("NOT_FOUND"),
            "must NOT have default 404 when custom fallback provided: {code}"
        );
    }

    #[test]
    fn feature_flag_on_non_async_fn_is_compile_error() {
        let result = feature_flag_macro(
            quote! { "my_flag" },
            quote! {
                fn my_sync_handler() -> &'static str {
                    "hello"
                }
            },
        );
        let code = result.to_string();
        assert!(
            code.contains("compile_error"),
            "non-async fn must produce compile_error: {code}"
        );
    }

    #[test]
    fn feature_flag_with_invalid_arg_is_compile_error() {
        let result = feature_flag_macro(
            quote! { "my_flag", unknown_arg = something },
            quote! {
                async fn my_handler() -> &'static str {
                    "hello"
                }
            },
        );
        let code = result.to_string();
        assert!(
            code.contains("compile_error"),
            "unknown arg must produce compile_error: {code}"
        );
    }
}
