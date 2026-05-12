//! `#[mailer_preview]` proc macro implementation.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItem, ImplItemFn, ItemImpl, Type};

struct PreviewMethod {
    method: syn::Ident,
    cfg_attrs: Vec<syn::Attribute>,
}

fn parse_preview_method(method: &ImplItemFn) -> syn::Result<Option<PreviewMethod>> {
    if !crate::mailer::returns_mail(method) {
        return Ok(None);
    }
    if method.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "#[mailer_preview] methods must be synchronous and return Mail",
        ));
    }
    if !method.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &method.sig.generics,
            "#[mailer_preview] methods must not have generics",
        ));
    }
    if method.sig.inputs.iter().any(|arg| match arg {
        FnArg::Receiver(_) | FnArg::Typed(_) => true,
    }) {
        return Err(syn::Error::new_spanned(
            &method.sig.inputs,
            "#[mailer_preview] methods must be zero-arg associated functions",
        ));
    }

    Ok(Some(PreviewMethod {
        method: method.sig.ident.clone(),
        cfg_attrs: method
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
            .cloned()
            .collect(),
    }))
}

fn mailer_name(self_ty: &Type) -> String {
    if let Type::Path(type_path) = self_ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident.to_string();
    }
    quote!(#self_ty).to_string().replace(' ', "")
}

pub fn mailer_preview_macro(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_impl: ItemImpl = match syn::parse2(item) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };

    let self_ty = input_impl.self_ty.clone();
    let input_generics = input_impl.generics.clone();
    let (impl_generics, _ty_generics, where_clause) = input_generics.split_for_impl();
    let impl_cfg_attrs = input_impl
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
        .cloned()
        .collect::<Vec<_>>();
    let mailer_name = mailer_name(&self_ty);

    let mut methods = Vec::new();
    for item in &input_impl.items {
        if let ImplItem::Fn(method) = item {
            match parse_preview_method(method) {
                Ok(Some(method)) => methods.push(method),
                Ok(None) => {}
                Err(err) => return err.to_compile_error(),
            }
        }
    }

    let registrations = methods.iter().map(|method| {
        let method_name = &method.method;
        let method_label = method_name.to_string();
        let cfg_attrs = &method.cfg_attrs;
        quote! {
            #( #cfg_attrs )*
            previews.push(::autumn_web::mail::MailPreview::new(
                #mailer_name,
                #method_label,
                Self::#method_name,
            ));
        }
    });

    quote! {
        #input_impl

        #( #impl_cfg_attrs )*
        impl #impl_generics #self_ty #where_clause {
            #[doc(hidden)]
            pub fn __autumn_mail_previews() -> ::std::vec::Vec<::autumn_web::mail::MailPreview> {
                let mut previews = ::std::vec::Vec::new();
                #( #registrations )*
                previews
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn generates_preview_registration_helper() {
        let out = mailer_preview_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn reset_preview() -> Mail {
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("__autumn_mail_previews"));
        assert!(rendered.contains("AccountMailer"));
        assert!(rendered.contains("reset_preview"));
    }

    #[test]
    fn rejects_preview_methods_with_arguments() {
        let out = mailer_preview_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn reset_preview(to: String) -> Mail {
                        let _ = to;
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        assert!(
            out.to_string()
                .contains("methods must be zero-arg associated functions")
        );
    }
}
