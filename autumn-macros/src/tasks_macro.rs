//! `tasks![]` collection macro.
//!
//! Collects `#[scheduled]`-annotated task handlers into a `Vec<TaskInfo>`,
//! parallel to the `routes![]` macro.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, Path, Token};

struct TaskList {
    tasks: Punctuated<Path, Token![,]>,
}

impl Parse for TaskList {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            tasks: Punctuated::parse_terminated(input)?,
        })
    }
}

pub fn tasks_macro(input: TokenStream) -> TokenStream {
    let task_list: TaskList = match syn::parse2(input) {
        Ok(tl) => tl,
        Err(err) => return err.to_compile_error(),
    };

    let calls: Vec<TokenStream> = task_list
        .tasks
        .iter()
        .map(|path| {
            // Convert task path to companion function: `my_task` → `__autumn_task_info_my_task`
            let mut companion = path.clone();
            if let Some(last) = companion.segments.last_mut() {
                let new_ident = Ident::new(
                    &format!("__autumn_task_info_{}", last.ident),
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
