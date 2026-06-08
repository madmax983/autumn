//! `#[secured]` proc macro implementation.
//!
//! Generates an authentication/authorization guard that runs before
//! the handler body. Injects hidden `Session` and `AppState` extractors
//! and prepends a call to the runtime check function.
//!
//! ## Forms
//!
//! - `#[secured]` -- require authenticated session (session key exists)
//! - `#[secured("admin")]` -- require a specific role
//! - `#[secured("admin", "editor")]` -- require any of the listed roles

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser as _;
use syn::{ItemFn, LitStr, parse_quote};

use crate::idempotency_guard::block_has_replay_guard;
use crate::param_helpers::has_input_named;

/// Parse the `#[secured(...)]` attribute arguments.
///
/// Returns a (possibly empty) list of role strings.
fn parse_secured_args(attr: TokenStream) -> syn::Result<Vec<String>> {
    if attr.is_empty() {
        return Ok(Vec::new());
    }

    let roles =
        syn::punctuated::Punctuated::<LitStr, syn::Token![,]>::parse_terminated.parse2(attr)?;
    Ok(roles.iter().map(LitStr::value).collect())
}

pub fn secured_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let roles = match parse_secured_args(attr) {
        Ok(r) => r,
        Err(err) => return err.to_compile_error(),
    };

    let mut input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "#[secured] can only be applied to async functions",
        )
        .to_compile_error();
    }

    let role_literals = roles.iter().map(|role| quote! { #role });
    let check_call = quote! {
        // Route macros read this marker when #[secured] expands before #[get]/#[post]/etc.
        const __AUTUMN_SECURED_ROLES: &[&str] = &[#(#role_literals),*];
        if let ::core::result::Result::Err(__autumn_error) = ::autumn_web::auth::__check_secured_with_key(
            &__autumn_session,
            __autumn_state.auth_session_key(),
            __AUTUMN_SECURED_ROLES,
        ).await {
            if __autumn_error.status() == ::autumn_web::reexports::http::StatusCode::UNAUTHORIZED {
                if let ::core::option::Option::Some(__autumn_response) =
                    ::autumn_web::idempotency::__replay_finalized_session_response(&__autumn_idempotency_replay)
                {
                    return __autumn_response;
                }
            }
            return ::autumn_web::reexports::axum::response::IntoResponse::into_response(__autumn_error);
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
    let body_already_has_replay_guard = block_has_replay_guard(original_body);
    let replay_stop = if body_already_has_replay_guard {
        quote! {}
    } else {
        quote! {
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            if let ::core::option::Option::Some(__autumn_response) =
                ::autumn_web::idempotency::__replay_response(&__autumn_idempotency_replay)
            {
                return __autumn_response;
            }
        }
    };

    // Inject hidden State<AppState> and Session parameters at the start of
    // the parameter list, but only if no other macro (typically `#[authorize]`) has
    // already injected them. Without these guards, stacking
    // `#[authorize]` + `#[secured]` in either attribute order would
    // produce duplicate hidden parameters and fail to
    // compile.
    if !has_input_named(&input_fn, "__autumn_state") {
        let state_param: syn::FnArg = syn::parse_quote! {
            ::autumn_web::reexports::axum::extract::State(__autumn_state):
                ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>
        };
        input_fn.sig.inputs.insert(0, state_param);
    }
    if !has_input_named(&input_fn, "__autumn_session") {
        let session_param: syn::FnArg = syn::parse_quote! {
            __autumn_session: ::autumn_web::session::Session
        };
        input_fn.sig.inputs.insert(0, session_param);
    }
    if !has_input_named(&input_fn, "__autumn_idempotency_replay") {
        let idempotency_param: syn::FnArg = syn::parse_quote! {
            __autumn_idempotency_replay: ::core::option::Option<
                ::autumn_web::reexports::axum::extract::Extension<
                    ::autumn_web::idempotency::IdempotencyReplayResponse
                >
            >
        };
        input_fn.sig.inputs.insert(0, idempotency_param);
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
            #replay_stop
            #original_response
        }
    };

    quote! { #input_fn }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::secured_macro;

    #[test]
    fn secured_string_literal_replay_guard_still_injects_replay_stop() {
        let generated = secured_macro(
            quote! {},
            quote! {
                async fn guarded() -> &'static str {
                    let _ = "__AUTUMN_IDEMPOTENCY_REPLAY_GUARD";
                    "ok"
                }
            },
        )
        .to_string();

        assert!(
            generated.contains("__replay_response"),
            "plain handler text must not suppress the generated replay stop: {generated}"
        );
    }
}
