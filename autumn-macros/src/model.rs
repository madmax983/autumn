//! `#[model]` attribute macro implementation.
//!
//! Emits Diesel + Serde derives and a `#[diesel(table_name)]` attribute
//! on the annotated struct.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser as _;
use syn::{DeriveInput, LitStr};

/// Process `#[model]` or `#[model(table = "...")]` attribute arguments.
///
/// Returns `Some(table_name)` if a `table = "..."` key was given, or
/// `None` to fall back to inference from the struct name.
fn parse_attr_args(attr: TokenStream) -> syn::Result<Option<String>> {
    if attr.is_empty() {
        return Ok(None);
    }

    let mut table = None;
    syn::meta::parser(|meta| {
        if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("unsupported model attribute"))
        }
    })
    .parse2(attr)?;

    Ok(table)
}

pub fn model_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    // Must be a struct
    let fields = match &input.data {
        syn::Data::Struct(data) => &data.fields,
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "#[model] can only be applied to structs",
            )
            .to_compile_error();
        }
    };

    // Extract table name from attribute args, or infer from struct name
    let table_name = match parse_attr_args(attr) {
        Ok(explicit) => explicit.unwrap_or_else(|| infer_table_name(&input.ident)),
        Err(err) => return err.to_compile_error(),
    };

    let table_ident = syn::Ident::new(&table_name, input.ident.span());

    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let attrs = &input.attrs;

    quote! {
        #[derive(Debug, Clone, ::diesel::Queryable, ::diesel::Selectable, ::diesel::Insertable)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #[diesel(table_name = #table_ident)]
        #(#attrs)*
        #vis struct #name #generics #fields
    }
}

fn infer_table_name(ident: &syn::Ident) -> String {
    let name = ident.to_string();
    let snake = pascal_to_snake(&name);
    format!("{snake}s")
}

fn pascal_to_snake(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_to_snake_simple() {
        assert_eq!(pascal_to_snake("User"), "user");
    }

    #[test]
    fn pascal_to_snake_multi_word() {
        assert_eq!(pascal_to_snake("BlogPost"), "blog_post");
    }

    #[test]
    fn pascal_to_snake_three_words() {
        assert_eq!(
            pascal_to_snake("UserProfileSettings"),
            "user_profile_settings"
        );
    }

    #[test]
    fn infer_table_name_simple() {
        let ident = syn::Ident::new("User", proc_macro2::Span::call_site());
        assert_eq!(infer_table_name(&ident), "users");
    }

    #[test]
    fn infer_table_name_multi_word() {
        let ident = syn::Ident::new("BlogPost", proc_macro2::Span::call_site());
        assert_eq!(infer_table_name(&ident), "blog_posts");
    }
}
