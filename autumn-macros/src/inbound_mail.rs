//! `#[inbound_mail(...)]` proc macro for registering inbound mail handlers.
//!
//! Annotates an async function to produce a companion `InboundMailHandlerInfo`
//! registration function.
//!
//! # Example
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::inbound_mail::{InboundEmail, InboundMailHandlerInfo,
//!     InboundMailRouter, ProcessingMode, RecipientPattern};
//!
//! #[inbound_mail(to = "support@company.com")]
//! async fn handle_support(email: InboundEmail) -> AutumnResult<()> {
//!     tracing::info!(from = %email.from, "inbound support email");
//!     Ok(())
//! }
//!
//! // The macro generates `handle_support_handler_info()` returning
//! // `InboundMailHandlerInfo` ready for registration.
//! autumn_web::app()
//!     .inbound_mail_router(
//!         InboundMailRouter::new()
//!             .endpoint(InboundMailEndpointConfig::mailgun("/inbound", "key"))
//!             .handler(handle_support_handler_info())
//!     )
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemFn, LitStr, parse::Parser as _};

/// Parsed attributes from `#[inbound_mail(...)]`.
struct InboundMailAttrs {
    /// Recipient address pattern string (from `to = "..."` attribute).
    to: Option<String>,
    /// Routing mode: "exact", "prefix", or "plus" (from `pattern = "..."` attribute).
    pattern_kind: Option<String>,
    /// Processing mode: "sync" or "background" (from `processing = "..."` attribute).
    processing: Option<String>,
}

fn parse_attrs(attr: TokenStream) -> syn::Result<InboundMailAttrs> {
    let mut result = InboundMailAttrs {
        to: None,
        pattern_kind: None,
        processing: None,
    };

    syn::meta::parser(|meta| {
        if meta.path.is_ident("to") {
            let value: LitStr = meta.value()?.parse()?;
            result.to = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("pattern") {
            let value: LitStr = meta.value()?.parse()?;
            result.pattern_kind = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("processing") {
            let value: LitStr = meta.value()?.parse()?;
            result.processing = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("unsupported attribute: expected `to`, `pattern`, or `processing`"))
        }
    })
    .parse2(attr)?;

    Ok(result)
}

/// Detect the routing pattern from the `to` attribute value.
///
/// - `"replies+{token}@app.example"` → `PlusAddress`
/// - `"prefix.*"` (ending in `*`) → `LocalPrefix`
/// - Any other string → `Exact`
fn detect_pattern(to: &str) -> TokenStream {
    if to == "*" || to.is_empty() {
        return quote! { ::autumn_web::inbound_mail::RecipientPattern::Any };
    }

    // Plus-address: `"{local}+{token}@{domain}"` or `"{local}+{token}"` (no domain).
    // The domain part is optional; `{token}` must be a literal `{...}` placeholder.
    let (local_part, domain_part) = if let Some(at_pos) = to.rfind('@') {
        (&to[..at_pos], Some(&to[at_pos + 1..]))
    } else {
        (to, None)
    };

    if let Some(plus_pos) = local_part.find('+') {
        let tag = &local_part[plus_pos + 1..];
        if tag.starts_with('{') && tag.ends_with('}') {
            let local = &local_part[..plus_pos];
            let domain = match domain_part {
                Some(d) if !d.is_empty() => {
                    let d = d.to_string();
                    quote! { Some(#d.to_string()) }
                }
                _ => quote! { None },
            };
            let l = local.to_string();
            return quote! {
                ::autumn_web::inbound_mail::RecipientPattern::PlusAddress {
                    local: #l.to_string(),
                    domain: #domain,
                }
            };
        }
    }

    // LocalPrefix: ends with `*` or `.*`.
    if to.ends_with('*') {
        let prefix = to
            .trim_end_matches('*')
            .trim_end_matches('.')
            .trim_end_matches('+');
        let p = prefix.to_string();
        return quote! {
            ::autumn_web::inbound_mail::RecipientPattern::LocalPrefix(#p.to_string())
        };
    }

    // Exact match by default.
    let addr = to.to_string();
    quote! {
        ::autumn_web::inbound_mail::RecipientPattern::Exact(#addr.to_string())
    }
}

/// Expand `#[inbound_mail(...)]` on an async function.
pub fn inbound_mail_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = match parse_attrs(attr) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };

    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &input_fn.sig.fn_token,
            "#[inbound_mail] functions must be async",
        )
        .to_compile_error();
    }

    let fn_name = &input_fn.sig.ident;
    let info_fn_name = format_ident!("{fn_name}_handler_info");
    let handler_name = fn_name.to_string();

    // Build the pattern token stream.
    let pattern_ts = if let Some(to) = &attrs.to {
        detect_pattern(to)
    } else {
        quote! { ::autumn_web::inbound_mail::RecipientPattern::Any }
    };

    // Build processing mode.
    let processing_ts = match attrs.processing.as_deref() {
        Some("sync") => quote! { ::autumn_web::inbound_mail::ProcessingMode::Sync },
        _ => quote! { ::autumn_web::inbound_mail::ProcessingMode::Background },
    };

    // Generate the wrapper function that adapts `async fn(InboundEmail) -> AutumnResult<()>`
    // to the `InboundMailHandlerFn` function pointer type.
    let wrapper_name = format_ident!("__inbound_mail_wrapper_{fn_name}");

    quote! {
        #input_fn

        fn #wrapper_name(
            email: ::autumn_web::inbound_mail::InboundEmail,
        ) -> ::std::pin::Pin<Box<
            dyn ::std::future::Future<
                Output = ::autumn_web::AutumnResult<()>
            > + Send + 'static
        >> {
            Box::pin(#fn_name(email))
        }

        /// Return the [`InboundMailHandlerInfo`] for this handler.
        ///
        /// Pass to [`InboundMailRouter::handler`] to register.
        #[must_use]
        pub fn #info_fn_name() -> ::autumn_web::inbound_mail::InboundMailHandlerInfo {
            ::autumn_web::inbound_mail::InboundMailHandlerInfo {
                name: #handler_name,
                pattern: #pattern_ts,
                processing: #processing_ts,
                handler: #wrapper_name,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn detect_exact_pattern() {
        let ts = detect_pattern("support@company.com");
        let s = ts.to_string();
        assert!(s.contains("Exact"), "expected Exact, got: {s}");
        assert!(s.contains("support@company.com"), "got: {s}");
    }

    #[test]
    fn detect_plus_address_pattern() {
        let ts = detect_pattern("replies+{token}@app.example");
        let s = ts.to_string();
        assert!(s.contains("PlusAddress"), "expected PlusAddress, got: {s}");
        assert!(s.contains("replies"), "got: {s}");
        assert!(s.contains("app.example"), "got: {s}");
    }

    #[test]
    fn detect_plus_address_no_domain() {
        let ts = detect_pattern("replies+{token}@");
        let s = ts.to_string();
        assert!(s.contains("PlusAddress"), "expected PlusAddress, got: {s}");
        assert!(s.contains("None"), "expected None domain, got: {s}");
    }

    #[test]
    fn detect_local_prefix_pattern() {
        let ts = detect_pattern("ticket+*");
        let s = ts.to_string();
        assert!(s.contains("LocalPrefix"), "expected LocalPrefix, got: {s}");
    }

    #[test]
    fn detect_any_pattern() {
        let ts = detect_pattern("*");
        let s = ts.to_string();
        assert!(s.contains("Any"), "expected Any, got: {s}");
    }

    #[test]
    fn parse_attrs_to() {
        let attr = quote! { to = "support@company.com" };
        let a = parse_attrs(attr).unwrap();
        assert_eq!(a.to.as_deref(), Some("support@company.com"));
    }

    #[test]
    fn parse_attrs_processing() {
        let attr = quote! { to = "a@b.com", processing = "sync" };
        let a = parse_attrs(attr).unwrap();
        assert_eq!(a.processing.as_deref(), Some("sync"));
    }

    #[test]
    fn parse_attrs_rejects_unknown() {
        let attr = quote! { unknown = "value" };
        let result = parse_attrs(attr);
        assert!(result.is_err());
    }

    #[test]
    fn macro_expands_on_valid_async_fn() {
        let attr = quote! { to = "support@company.com" };
        let item = quote! {
            async fn handle_support(
                email: ::autumn_web::inbound_mail::InboundEmail,
            ) -> ::autumn_web::AutumnResult<()> {
                Ok(())
            }
        };
        let expanded = inbound_mail_macro(attr, item);
        let s = expanded.to_string();
        assert!(
            s.contains("handle_support_handler_info"),
            "expected handler info fn, got: {s}"
        );
        assert!(s.contains("Exact"), "expected Exact pattern, got: {s}");
    }

    #[test]
    fn macro_rejects_non_async_fn() {
        let attr = quote! {};
        let item = quote! {
            fn not_async(email: InboundEmail) -> AutumnResult<()> {
                Ok(())
            }
        };
        let expanded = inbound_mail_macro(attr, item);
        let s = expanded.to_string();
        assert!(
            s.contains("compile_error"),
            "expected compile_error, got: {s}"
        );
    }

    #[test]
    fn macro_generates_plus_address_pattern() {
        let attr = quote! { to = "replies+{token}@app.example" };
        let item = quote! {
            async fn handle_reply(
                email: ::autumn_web::inbound_mail::InboundEmail,
            ) -> ::autumn_web::AutumnResult<()> {
                Ok(())
            }
        };
        let expanded = inbound_mail_macro(attr, item);
        let s = expanded.to_string();
        assert!(s.contains("PlusAddress"), "expected PlusAddress, got: {s}");
    }
}
