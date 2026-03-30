use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, PathArguments, Type};

pub fn island_macro(item: TokenStream) -> TokenStream {
    let input: ItemFn = match syn::parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    if input.sig.asyncness.is_some() {
        return syn::Error::new_spanned(
            &input.sig.fn_token,
            "#[island] does not support async fn; use a synchronous function",
        )
        .to_compile_error();
    }

    let Some(first_arg) = input.sig.inputs.first() else {
        return syn::Error::new_spanned(
            &input.sig.ident,
            "#[island] requires first argument like IslandCx<Props>",
        )
        .to_compile_error();
    };

    let props_type = match first_arg {
        FnArg::Typed(pat) => match &*pat.ty {
            Type::Path(type_path) => {
                let Some(last) = type_path.path.segments.last() else {
                    return syn::Error::new_spanned(&pat.ty, "invalid IslandCx type")
                        .to_compile_error();
                };
                if last.ident != "IslandCx" {
                    return syn::Error::new_spanned(&pat.ty, "expected IslandCx<Props>")
                        .to_compile_error();
                }
                let PathArguments::AngleBracketed(args) = &last.arguments else {
                    return syn::Error::new_spanned(&pat.ty, "expected IslandCx<Props>")
                        .to_compile_error();
                };
                let Some(syn::GenericArgument::Type(props)) = args.args.first() else {
                    return syn::Error::new_spanned(&pat.ty, "expected IslandCx<Props>")
                        .to_compile_error();
                };
                props.clone()
            }
            _ => {
                return syn::Error::new_spanned(&pat.ty, "expected IslandCx<Props>")
                    .to_compile_error();
            }
        },
        FnArg::Receiver(_) => {
            return syn::Error::new_spanned(first_arg, "#[island] does not support methods")
                .to_compile_error();
        }
    };

    let name = &input.sig.ident;
    let meta_name = format_ident!("__autumn_island_meta_{}", name);

    quote! {
        #input

        #[doc(hidden)]
        pub fn #meta_name() -> ::autumn_web::wasm::IslandMeta {
            ::autumn_web::wasm::IslandMeta {
                name: stringify!(#name),
                mount_id: stringify!(#name),
                props_type: ::core::any::type_name::<#props_type>(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn rejects_async_island_fn() {
        let out = island_macro(quote! {
            async fn counter(cx: IslandCx<Props>) {}
        });
        assert!(out.to_string().contains("does not support async fn"));
    }

    #[test]
    fn accepts_sync_island_fn() {
        let out = island_macro(quote! {
            fn counter(cx: IslandCx<Props>) {}
        });
        let out = out.to_string();
        assert!(out.contains("fn counter"));
        assert!(out.contains("__autumn_island_meta_counter"));
    }
}
