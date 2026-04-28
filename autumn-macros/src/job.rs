//! `#[job]` proc macro implementation.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{FnArg, ItemFn, LitInt, LitStr, PatType, Type};

struct JobAttrs {
    name: Option<String>,
    max_attempts: Option<u32>,
    backoff_ms: Option<u64>,
}

fn parse_job_args(attr: TokenStream) -> syn::Result<JobAttrs> {
    let mut result = JobAttrs {
        name: None,
        max_attempts: None,
        backoff_ms: None,
    };

    syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            result.name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("max_attempts") {
            let value: LitInt = meta.value()?.parse()?;
            result.max_attempts = Some(value.base10_parse::<u32>()?);
            Ok(())
        } else if meta.path.is_ident("backoff_ms") {
            let value: LitInt = meta.value()?.parse()?;
            result.backoff_ms = Some(value.base10_parse::<u64>()?);
            Ok(())
        } else {
            Err(meta.error("unsupported attribute: expected name, max_attempts, or backoff_ms"))
        }
    })
    .parse2(attr)?;

    Ok(result)
}

fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<String>()
}

pub fn job_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = match parse_job_args(attr) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error(),
    };

    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(input_fn.sig.fn_token, "#[job] functions must be async")
            .to_compile_error();
    }

    if input_fn.sig.inputs.len() != 2 {
        return syn::Error::new_spanned(
            &input_fn.sig.ident,
            "#[job] function must have signature async fn(AppState, Args)",
        )
        .to_compile_error();
    }

    let mut inputs = input_fn.sig.inputs.iter();
    let _state_arg = inputs.next();
    let args_type: Type = match inputs.next() {
        Some(FnArg::Typed(PatType { ty, .. })) => (**ty).clone(),
        _ => {
            return syn::Error::new_spanned(
                &input_fn.sig.ident,
                "#[job] second argument must be a typed args struct",
            )
            .to_compile_error();
        }
    };

    let fn_name = &input_fn.sig.ident;
    let companion_name = format_ident!("__autumn_job_info_{fn_name}");
    let api_name = format_ident!("{}Job", pascal_case(&fn_name.to_string()));
    let job_name = attrs.name.unwrap_or_else(|| fn_name.to_string());
    let max_attempts = attrs.max_attempts.unwrap_or(0);
    let backoff_ms = attrs.backoff_ms.unwrap_or(0);

    quote! {
        #input_fn

        pub struct #api_name;

        impl #api_name {
            pub async fn enqueue(args: #args_type) -> ::autumn_web::AutumnResult<()> {
                let payload = ::autumn_web::reexports::serde_json::to_value(&args)
                    .map_err(|e| ::autumn_web::AutumnError::internal_server_error(::std::io::Error::other(format!("job args serialization failed: {e}"))))?;
                ::autumn_web::job::enqueue(#job_name, payload).await
            }
        }

        #[doc(hidden)]
        pub fn #companion_name() -> ::autumn_web::job::JobInfo {
            ::autumn_web::job::JobInfo {
                name: #job_name.to_string(),
                max_attempts: #max_attempts,
                initial_backoff_ms: #backoff_ms,
                handler: |state: ::autumn_web::AppState, payload: ::autumn_web::reexports::serde_json::Value| {
                    Box::pin(async move {
                        let args: #args_type = ::autumn_web::reexports::serde_json::from_value(payload)
                            .map_err(|e| ::autumn_web::AutumnError::internal_server_error(::std::io::Error::other(format!("job args deserialization failed: {e}"))))?;
                        #fn_name(state, args).await
                    })
                },
            }
        }
    }
}
