//! `#[cached]` proc macro implementation.
//!
//! Wraps an async (or sync) function with an in-memory cache backed by
//! `autumn_web::cache::CacheStore`. Each annotated function gets its own
//! `static` cache instance, keyed by a tuple of the function arguments.
//!
//! # Supported attributes
//!
//! | Attribute | Example | Description |
//! |-----------|---------|-------------|
//! | `ttl` | `"5m"` | Time-to-live per entry (parsed at startup) |
//! | `max` | `1000` | Max entries; oldest evicted on overflow |
//! | `result` | (flag) | Only cache `Ok` values; pass `Err` through |

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser as _;
use syn::{Expr, ItemFn, LitInt, LitStr};

struct CachedAttrs {
    ttl: Option<String>,
    max: Option<usize>,
    result: bool,
}

/// Try to parse `max` as either a string literal or an integer literal.
fn parse_max_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<usize> {
    let expr: Expr = meta.value()?.parse()?;
    match &expr {
        Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Int(int) => int.base10_parse::<usize>(),
            syn::Lit::Str(s) => s.value().parse::<usize>().map_err(|_| {
                syn::Error::new_spanned(s, "max must be a positive integer")
            }),
            _ => Err(syn::Error::new_spanned(&expr, "max must be an integer")),
        },
        _ => Err(syn::Error::new_spanned(&expr, "max must be a literal integer")),
    }
}

fn parse_cached_args(attr: TokenStream) -> syn::Result<CachedAttrs> {
    let mut result = CachedAttrs {
        ttl: None,
        max: None,
        result: false,
    };

    if attr.is_empty() {
        return Ok(result);
    }

    syn::meta::parser(|meta| {
        if meta.path.is_ident("ttl") {
            let value: LitStr = meta.value()?.parse()?;
            result.ttl = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("max") {
            result.max = Some(parse_max_value(&meta)?);
            Ok(())
        } else if meta.path.is_ident("result") {
            result.result = true;
            Ok(())
        } else {
            Err(meta.error("unsupported attribute: expected ttl, max, or result"))
        }
    })
    .parse2(attr)?;

    Ok(result)
}

pub fn cached_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = match parse_cached_args(attr) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error(),
    };

    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    let vis = &input_fn.vis;
    let sig = &input_fn.sig;
    let fn_name = &sig.ident;
    let fn_attrs = &input_fn.attrs;
    let fn_block = &input_fn.block;
    let ret_ty = &sig.output;
    let is_async = sig.asyncness.is_some();

    // Collect function parameters for cache key construction.
    // We need the parameter names and types for the key tuple.
    let mut param_names = Vec::new();
    let mut param_types = Vec::new();
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    arg,
                    "#[cached] does not support methods with `self`",
                )
                .to_compile_error();
            }
            syn::FnArg::Typed(pat_type) => {
                param_names.push(&*pat_type.pat);
                param_types.push(&*pat_type.ty);
            }
        }
    }

    // Build the key type as a tuple: (T1, T2, ...)
    let key_type = if param_types.is_empty() {
        quote! { () }
    } else {
        quote! { (#(#param_types,)*) }
    };

    // Build the key expression: (arg1.clone(), arg2.clone(), ...)
    let key_expr = if param_names.is_empty() {
        quote! { () }
    } else {
        quote! { (#(#param_names.clone(),)*) }
    };

    // Build the value type from the return type.
    // For `result` mode, we use the CacheableResult trait to extract
    // the Ok type at the type level, avoiding syntactic parsing of generics.
    let ret_type = match ret_ty {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, ty) => quote! { #ty },
    };

    let value_type = if attrs.result {
        quote! { <#ret_type as ::autumn_web::cache::CacheableResult>::Ok }
    } else {
        ret_type.clone()
    };

    // TTL expression
    let ttl_expr = match &attrs.ttl {
        Some(ttl) => {
            let ttl_str = ttl.clone();
            quote! {
                Some(
                    ::autumn_web::task::parse_duration(#ttl_str)
                        .expect(concat!("invalid duration in #[cached(ttl = \"", #ttl_str, "\")]"))
                )
            }
        }
        None => quote! { None },
    };

    // Max expression
    let max_expr = match attrs.max {
        Some(max) => {
            let max_lit = LitInt::new(&max.to_string(), proc_macro2::Span::call_site());
            quote! { Some(#max_lit) }
        }
        None => quote! { None },
    };

    // Generate the body depending on result mode and async.
    let body = if attrs.result {
        // Result mode: only cache Ok values via the CacheableResult trait.
        let compute = if is_async {
            quote! { (|| async move #fn_block)().await }
        } else {
            quote! { (|| #fn_block)() }
        };
        quote! {
            static __AUTUMN_CACHE: ::std::sync::OnceLock<
                ::autumn_web::cache::CacheStore<#key_type, #value_type>
            > = ::std::sync::OnceLock::new();
            let __autumn_cache = __AUTUMN_CACHE.get_or_init(|| {
                ::autumn_web::cache::CacheStore::new(#ttl_expr, #max_expr)
            });
            let __autumn_key = #key_expr;
            if let Some(__autumn_cached) = __autumn_cache.get(&__autumn_key) {
                return <#ret_type as ::autumn_web::cache::CacheableResult>::from_ok(__autumn_cached);
            }
            let __autumn_result = #compute;
            match <#ret_type as ::autumn_web::cache::CacheableResult>::into_result(__autumn_result) {
                Ok(__autumn_val) => {
                    __autumn_cache.insert(__autumn_key, __autumn_val.clone());
                    <#ret_type as ::autumn_web::cache::CacheableResult>::from_ok(__autumn_val)
                }
                Err(__autumn_err) => Err(__autumn_err),
            }
        }
    } else {
        // Standard mode: cache the full return value.
        if is_async {
            quote! {
                static __AUTUMN_CACHE: ::std::sync::OnceLock<
                    ::autumn_web::cache::CacheStore<#key_type, #value_type>
                > = ::std::sync::OnceLock::new();
                let __autumn_cache = __AUTUMN_CACHE.get_or_init(|| {
                    ::autumn_web::cache::CacheStore::new(#ttl_expr, #max_expr)
                });
                let __autumn_key = #key_expr;
                if let Some(__autumn_cached) = __autumn_cache.get(&__autumn_key) {
                    return __autumn_cached;
                }
                let __autumn_result = (|| async move #fn_block)().await;
                __autumn_cache.insert(__autumn_key, __autumn_result.clone());
                __autumn_result
            }
        } else {
            quote! {
                static __AUTUMN_CACHE: ::std::sync::OnceLock<
                    ::autumn_web::cache::CacheStore<#key_type, #value_type>
                > = ::std::sync::OnceLock::new();
                let __autumn_cache = __AUTUMN_CACHE.get_or_init(|| {
                    ::autumn_web::cache::CacheStore::new(#ttl_expr, #max_expr)
                });
                let __autumn_key = #key_expr;
                if let Some(__autumn_cached) = __autumn_cache.get(&__autumn_key) {
                    return __autumn_cached;
                }
                let __autumn_result = (|| #fn_block)();
                __autumn_cache.insert(__autumn_key, __autumn_result.clone());
                __autumn_result
            }
        }
    };

    // Rebuild the function with the caching wrapper body.
    quote! {
        #(#fn_attrs)*
        #vis #sig {
            #body
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_attrs() {
        let attrs = parse_cached_args(TokenStream::new()).unwrap();
        assert!(attrs.ttl.is_none());
        assert!(attrs.max.is_none());
        assert!(!attrs.result);
    }

    #[test]
    fn parse_ttl_only() {
        let tokens: TokenStream = quote! { ttl = "5m" };
        let attrs = parse_cached_args(tokens).unwrap();
        assert_eq!(attrs.ttl.as_deref(), Some("5m"));
        assert!(attrs.max.is_none());
        assert!(!attrs.result);
    }

    #[test]
    fn parse_all_attrs() {
        let tokens: TokenStream = quote! { ttl = "1h", max = 100, result };
        let attrs = parse_cached_args(tokens).unwrap();
        assert_eq!(attrs.ttl.as_deref(), Some("1h"));
        assert_eq!(attrs.max, Some(100));
        assert!(attrs.result);
    }

    #[test]
    fn parse_max_as_integer() {
        let tokens: TokenStream = quote! { max = 500 };
        let attrs = parse_cached_args(tokens).unwrap();
        assert_eq!(attrs.max, Some(500));
    }

    #[test]
    fn parse_result_flag_only() {
        let tokens: TokenStream = quote! { result };
        let attrs = parse_cached_args(tokens).unwrap();
        assert!(attrs.result);
    }

    #[test]
    fn parse_unknown_attr_errors() {
        let tokens: TokenStream = quote! { foo = "bar" };
        assert!(parse_cached_args(tokens).is_err());
    }

    #[test]
    fn generated_output_contains_cache_store() {
        let attr: TokenStream = quote! { ttl = "5m" };
        let item: TokenStream = quote! {
            async fn get_user(id: i64) -> String {
                format!("user-{id}")
            }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(output_str.contains("CacheStore"), "should reference CacheStore");
        assert!(output_str.contains("OnceLock"), "should use OnceLock for static");
    }

    #[test]
    fn generated_output_result_mode() {
        let attr: TokenStream = quote! { result };
        let item: TokenStream = quote! {
            async fn get_user(id: i64) -> Result<String, Error> {
                Ok(format!("user-{id}"))
            }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(
            output_str.contains("Ok"),
            "result mode should pattern-match on Ok"
        );
    }

    #[test]
    fn no_args_function() {
        let attr: TokenStream = quote! {};
        let item: TokenStream = quote! {
            async fn get_config() -> Vec<String> {
                vec!["a".into()]
            }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(output_str.contains("CacheStore"), "should still generate cache");
    }

    #[test]
    fn self_receiver_errors() {
        let attr: TokenStream = quote! {};
        let item: TokenStream = quote! {
            async fn get_thing(&self) -> String {
                "hi".into()
            }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(
            output_str.contains("compile_error"),
            "should produce compile error for self"
        );
    }
}
