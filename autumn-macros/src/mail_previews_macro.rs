//! `mail_previews![]` proc macro implementation.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Token, Type};

pub fn mail_previews_macro(input: TokenStream) -> TokenStream {
    let types = match Punctuated::<Type, Token![,]>::parse_terminated.parse2(input) {
        Ok(types) => types,
        Err(err) => return err.to_compile_error(),
    };

    let registrations = types.iter().map(|ty| {
        quote! {
            previews.extend(#ty::__autumn_mail_previews());
        }
    });

    quote! {{
        let mut previews = ::std::vec::Vec::new();
        #( #registrations )*
        previews
    }}
}
