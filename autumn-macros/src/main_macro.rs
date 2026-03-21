//! `#[autumn::main]` macro implementation.
//!
//! A thin wrapper around `#[tokio::main]` that sets up the async runtime
//! for an Autumn application.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemFn;

pub fn main_macro(item: TokenStream) -> TokenStream {
    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(input_fn.sig.fn_token, "the main function must be async")
            .to_compile_error();
    }

    quote! {
        #[::autumn::reexports::tokio::main]
        #input_fn
    }
}
