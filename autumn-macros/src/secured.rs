//! `#[secured]` proc macro implementation.
//!
//! Generates an authentication/authorization guard that runs before
//! the handler body. Injects a hidden `Session` extractor and prepends
//! a call to the runtime check function.
//!
//! ## Forms
//!
//! - `#[secured]` -- require authenticated session (session key exists)
//! - `#[secured("admin")]` -- require a specific role
//! - `#[secured("admin", "editor")]` -- require any of the listed roles

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser as _;
use syn::{ItemFn, LitStr};

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

    // Build the roles slice expression
    let check_call = if roles.is_empty() {
        quote! {
            ::autumn_web::auth::__check_secured(&__autumn_session, &[]).await?;
        }
    } else {
        let role_literals = roles.iter().map(|r| quote! { #r });
        quote! {
            ::autumn_web::auth::__check_secured(&__autumn_session, &[#(#role_literals),*]).await?;
        }
    };

    // Inject the hidden Session parameter at the start of the parameter
    // list — but only if no other macro (typically `#[authorize]`) has
    // already injected one. Without this guard, stacking
    // `#[authorize]` + `#[secured]` in either attribute order would
    // produce duplicate `__autumn_session` parameters and fail to
    // compile.
    if !has_input_named(&input_fn, "__autumn_session") {
        let session_param: syn::FnArg = syn::parse_quote! {
            __autumn_session: ::autumn_web::session::Session
        };
        input_fn.sig.inputs.insert(0, session_param);
    }

    // Prepend the check call to the function body
    let original_body = &input_fn.block;
    input_fn.block = syn::parse_quote! {
        {
            #check_call
            #original_body
        }
    };

    quote! { #input_fn }
}
