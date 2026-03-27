//! `static_routes![]` collection macro.
//!
//! Collects `#[static_get]`-annotated handlers into a `Vec<StaticRouteMeta>`,
//! parallel to the `routes![]` and `tasks![]` macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, Path, Token};

struct StaticRouteList {
    routes: Punctuated<Path, Token![,]>,
}

impl Parse for StaticRouteList {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            routes: Punctuated::parse_terminated(input)?,
        })
    }
}

pub fn static_routes_macro(input: TokenStream) -> TokenStream {
    let route_list: StaticRouteList = match syn::parse2(input) {
        Ok(rl) => rl,
        Err(err) => return err.to_compile_error(),
    };

    let calls: Vec<TokenStream> = route_list
        .routes
        .iter()
        .map(|path| {
            // Convert handler path to companion function: `about` -> `__autumn_static_meta_about`
            let mut companion = path.clone();
            if let Some(last) = companion.segments.last_mut() {
                let new_ident = Ident::new(
                    &format!("__autumn_static_meta_{}", last.ident),
                    last.ident.span(),
                );
                last.ident = new_ident;
            }
            quote! { #companion() }
        })
        .collect();

    quote! {
        vec![#(#calls),*]
    }
}
