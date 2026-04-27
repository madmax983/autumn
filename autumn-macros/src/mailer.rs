//! `#[mailer]` proc macro implementation.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItem, ImplItemFn, ItemImpl, Pat, ReturnType, Type};

struct MailMethod {
    method: syn::Ident,
    send_method: syn::Ident,
    later_method: syn::Ident,
    args: Vec<(syn::Ident, Type)>,
}

fn parse_mail_method(method: &ImplItemFn) -> syn::Result<Option<MailMethod>> {
    if method.sig.receiver().is_none() {
        return Ok(None);
    }
    if method.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "#[mailer] template methods must be synchronous and return Mail",
        ));
    }
    if matches!(method.sig.output, ReturnType::Default) {
        return Ok(None);
    }

    let mut args = Vec::new();
    for arg in &method.sig.inputs {
        match arg {
            FnArg::Receiver(_) => {}
            FnArg::Typed(pat_type) => {
                let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
                    return Err(syn::Error::new_spanned(
                        &pat_type.pat,
                        "#[mailer] method arguments must be simple identifiers",
                    ));
                };
                args.push((pat_ident.ident.clone(), (*pat_type.ty).clone()));
            }
        }
    }

    let method_name = &method.sig.ident;
    Ok(Some(MailMethod {
        method: method_name.clone(),
        send_method: format_ident!("send_{method_name}"),
        later_method: format_ident!("deliver_later_{method_name}"),
        args,
    }))
}

pub fn mailer_macro(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_impl: ItemImpl = match syn::parse2(item) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };

    let self_ty = input_impl.self_ty.clone();
    let mut methods = Vec::new();
    for item in &input_impl.items {
        if let ImplItem::Fn(method) = item {
            match parse_mail_method(method) {
                Ok(Some(method)) => methods.push(method),
                Ok(None) => {}
                Err(err) => return err.to_compile_error(),
            }
        }
    }

    let generated = methods.iter().map(|method| {
        let original = &method.method;
        let send_method = &method.send_method;
        let later_method = &method.later_method;
        let arg_defs = method.args.iter().map(|(name, ty)| quote! { #name: #ty });
        let arg_defs_later = method.args.iter().map(|(name, ty)| quote! { #name: #ty });
        let arg_names = method.args.iter().map(|(name, _)| quote! { #name });
        let arg_names_later = method.args.iter().map(|(name, _)| quote! { #name });
        quote! {
            pub async fn #send_method(
                &self,
                mailer: &::autumn_web::mail::Mailer,
                #( #arg_defs, )*
            ) -> ::autumn_web::AutumnResult<()> {
                let mail = self.#original( #( #arg_names, )* );
                mailer.send(mail).await.map_err(::autumn_web::AutumnError::internal_server_error)
            }

            pub fn #later_method(
                &self,
                mailer: &::autumn_web::mail::Mailer,
                #( #arg_defs_later, )*
            ) {
                let mail = self.#original( #( #arg_names_later, )* );
                mailer.deliver_later(mail);
            }
        }
    });

    quote! {
        #input_impl

        impl #self_ty {
            #( #generated )*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn generates_send_and_later_helpers() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn reset(&self, to: String) -> Mail {
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("send_reset"));
        assert!(rendered.contains("deliver_later_reset"));
    }
}
