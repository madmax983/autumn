//! `#[cached]` proc macro implementation.
//!
//! Wraps an async (or sync) function with an in-memory cache backed by
//! `autumn_web::cache::MokaCache` (default) via the `autumn_web::cache::Cache`
//! trait. Each annotated function gets its own `static` cache instance,
//! keyed by a hash of the function arguments.
//!
//! # Supported attributes
//!
//! | Attribute | Example | Description |
//! |-----------|---------|-------------|
//! | `ttl` | `"5m"` | Time-to-live per entry (parsed at startup) |
//! | `max` | `1000` | Max entries; LRU eviction via moka |
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
            syn::Lit::Str(s) => s
                .value()
                .parse::<usize>()
                .map_err(|_| syn::Error::new_spanned(s, "max must be a positive integer")),
            _ => Err(syn::Error::new_spanned(&expr, "max must be an integer")),
        },
        _ => Err(syn::Error::new_spanned(
            &expr,
            "max must be a literal integer",
        )),
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

/// Generate the cache wrapper body for a single function.
fn generate_cache_body(
    attrs: &CachedAttrs,
    fn_name_str: &str,
    fn_block: &syn::Block,
    is_async: bool,
    key_args: &TokenStream,
    ret_type: &TokenStream,
    value_type: &TokenStream,
) -> TokenStream {
    let ttl_expr = attrs.ttl.as_ref().map_or_else(
        || quote! { None },
        |ttl| {
            let ttl_str = ttl.clone();
            quote! {
                Some(
                    ::autumn_web::task::parse_duration(#ttl_str)
                        .expect(concat!("invalid duration in #[cached(ttl = \"", #ttl_str, "\")]"))
                )
            }
        },
    );

    let max_expr = attrs.max.map_or_else(
        || quote! { 10_000 },
        |max| {
            let max_lit = LitInt::new(&max.to_string(), proc_macro2::Span::call_site());
            quote! { #max_lit }
        },
    );

    let compute = if is_async {
        quote! { (|| async move #fn_block)().await }
    } else {
        quote! { (|| #fn_block)() }
    };

    let cache_init = quote! {
        static __AUTUMN_CACHE: ::std::sync::OnceLock<
            ::autumn_web::cache::MokaCache
        > = ::std::sync::OnceLock::new();
        let __autumn_moka = __AUTUMN_CACHE.get_or_init(|| {
            ::autumn_web::cache::MokaCache::new(#max_expr, #ttl_expr)
        });
        // Use the process-level shared backend when registered, otherwise fall
        // back to the per-function Moka store so zero-config local dev still works.
        let __autumn_global = ::autumn_web::cache::global_cache();
        let __autumn_cache: &dyn ::autumn_web::cache::Cache =
            __autumn_global
                .as_deref()
                .unwrap_or(__autumn_moka as &dyn ::autumn_web::cache::Cache);
        let __autumn_key = ::autumn_web::cache::make_cache_key(#fn_name_str, #key_args);
    };

    if attrs.result {
        quote! {
            #cache_init
            if let Some(__autumn_cached) = ::autumn_web::cache::get::<#value_type>(__autumn_cache, &__autumn_key) {
                return <#ret_type as ::autumn_web::cache::CacheableResult>::from_ok(__autumn_cached);
            }
            let __autumn_result = #compute;
            match <#ret_type as ::autumn_web::cache::CacheableResult>::into_result(__autumn_result) {
                Ok(__autumn_val) => {
                    ::autumn_web::cache::insert::<#value_type>(__autumn_cache, &__autumn_key, __autumn_val.clone());
                    <#ret_type as ::autumn_web::cache::CacheableResult>::from_ok(__autumn_val)
                }
                Err(__autumn_err) => Err(__autumn_err),
            }
        }
    } else {
        quote! {
            #cache_init
            if let Some(__autumn_cached) = ::autumn_web::cache::get::<#value_type>(__autumn_cache, &__autumn_key) {
                return __autumn_cached;
            }
            let __autumn_result = #compute;
            ::autumn_web::cache::insert::<#value_type>(__autumn_cache, &__autumn_key, __autumn_result.clone());
            __autumn_result
        }
    }
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
    let fn_name_str = fn_name.to_string();
    let fn_attrs = &input_fn.attrs;
    let fn_block = &input_fn.block;
    let is_async = sig.asyncness.is_some();

    // Collect function parameters for cache key construction.
    let mut param_names = Vec::new();
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
            }
        }
    }

    let key_args = if param_names.is_empty() {
        quote! { &() }
    } else {
        quote! { &(#(#param_names.clone(),)*) }
    };

    let ret_type = match &sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, ty) => quote! { #ty },
    };

    let value_type = if attrs.result {
        quote! { <#ret_type as ::autumn_web::cache::CacheableResult>::Ok }
    } else {
        ret_type.clone()
    };

    let body = generate_cache_body(
        &attrs,
        &fn_name_str,
        fn_block,
        is_async,
        &key_args,
        &ret_type,
        &value_type,
    );

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
    fn generated_output_uses_moka() {
        let attr: TokenStream = quote! { ttl = "5m" };
        let item: TokenStream = quote! {
            async fn get_user(id: i64) -> String {
                format!("user-{id}")
            }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(
            output_str.contains("MokaCache"),
            "should reference MokaCache"
        );
        assert!(
            output_str.contains("make_cache_key"),
            "should use make_cache_key"
        );
        assert!(
            output_str.contains("OnceLock"),
            "should use OnceLock for static"
        );
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
            output_str.contains("CacheableResult"),
            "result mode should use CacheableResult trait"
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
        assert!(
            output_str.contains("MokaCache"),
            "should still generate cache"
        );
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

    #[test]
    fn default_max_capacity() {
        let attr: TokenStream = quote! {};
        let item: TokenStream = quote! {
            fn compute(x: i32) -> i32 { x }
        };
        let output = cached_macro(attr, item);
        let output_str = output.to_string();
        assert!(
            output_str.contains("10_000"),
            "default max should be 10_000"
        );
    }
}
