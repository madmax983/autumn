//! `#[mailer]` proc macro implementation.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, GenericParam, ImplItem, ImplItemFn, ItemImpl, Pat, ReturnType, Type};

struct MailMethod {
    method: syn::Ident,
    send_method: syn::Ident,
    later_method: syn::Ident,
    generics: syn::Generics,
    args: Vec<(syn::Ident, Type)>,
}

fn returns_mail(method: &ImplItemFn) -> bool {
    let ReturnType::Type(_, ty) = &method.sig.output else {
        return false;
    };
    let Type::Path(type_path) = ty.as_ref() else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Mail")
}

fn parse_mail_method(method: &ImplItemFn) -> syn::Result<Option<MailMethod>> {
    if !returns_mail(method) {
        return Ok(None);
    }
    let Some(receiver) = method.sig.receiver() else {
        return Ok(None);
    };
    if receiver.reference.is_none() || receiver.mutability.is_some() {
        return Err(syn::Error::new_spanned(
            receiver,
            "#[mailer] template methods must use an `&self` receiver",
        ));
    }
    if method.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "#[mailer] template methods must be synchronous and return Mail",
        ));
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
        generics: method.sig.generics.clone(),
        args,
    }))
}

pub fn mailer_macro(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_impl: ItemImpl = match syn::parse2(item) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };

    let self_ty = input_impl.self_ty.clone();
    let input_generics = input_impl.generics.clone();
    let (impl_generics, _ty_generics, where_clause) = input_generics.split_for_impl();
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
        let method_generic_params = &method.generics.params;
        let method_generic_decl = if method_generic_params.is_empty() {
            quote! {}
        } else {
            quote! { <#method_generic_params> }
        };
        let method_generic_call_args = method
            .generics
            .params
            .iter()
            .filter_map(|param| match param {
                GenericParam::Type(param) => {
                    let ident = &param.ident;
                    Some(quote! { #ident })
                }
                GenericParam::Lifetime(_) => None,
                GenericParam::Const(param) => {
                    let ident = &param.ident;
                    Some(quote! { #ident })
                }
            })
            .collect::<Vec<_>>();
        let method_generic_call = if method_generic_call_args.is_empty() {
            quote! {}
        } else {
            quote! { ::<#(#method_generic_call_args),*> }
        };
        let method_where_clause = &method.generics.where_clause;
        let arg_defs = method.args.iter().map(|(name, ty)| quote! { #name: #ty });
        let arg_defs_later = method.args.iter().map(|(name, ty)| quote! { #name: #ty });
        let arg_names = method.args.iter().map(|(name, _)| quote! { #name });
        let arg_names_later = method.args.iter().map(|(name, _)| quote! { #name });
        quote! {
            pub async fn #send_method #method_generic_decl (
                &self,
                mailer: &::autumn_web::mail::Mailer,
                #( #arg_defs, )*
            ) -> ::autumn_web::AutumnResult<()>
            #method_where_clause
            {
                let mail = self.#original #method_generic_call ( #( #arg_names, )* );
                mailer.send(mail).await.map_err(::autumn_web::AutumnError::internal_server_error)
            }

            pub fn #later_method #method_generic_decl (
                &self,
                mailer: &::autumn_web::mail::Mailer,
                #( #arg_defs_later, )*
            )
            #method_where_clause
            {
                let mail = self.#original #method_generic_call ( #( #arg_names_later, )* );
                mailer.deliver_later(mail);
            }
        }
    });

    quote! {
        #input_impl

        impl #impl_generics #self_ty #where_clause {
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

    #[test]
    fn preserves_impl_generics_and_where_clause() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl<T> AccountMailer<T>
                where
                    T: Clone,
                {
                    fn reset(&self, to: String) -> Mail {
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("impl < T > AccountMailer < T > where T : Clone"));
        assert!(rendered.contains("send_reset"));
    }

    #[test]
    fn preserves_method_generics_and_where_clause() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn welcome<T>(&self, to: T) -> Mail
                    where
                        T: std::fmt::Display,
                    {
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("pub async fn send_welcome < T >"));
        assert!(rendered.contains("where T : std :: fmt :: Display"));
        assert!(rendered.contains("pub fn deliver_later_welcome < T >"));
    }

    #[test]
    fn forwards_method_generics_into_helper_calls() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn welcome<T>(&self, to: String) -> Mail
                    where
                        T: Default,
                    {
                        let _ = T::default();
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("self . welcome :: < T >"));
    }

    #[test]
    fn does_not_forward_lifetime_generics_into_helper_calls() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn welcome<'a>(&self, to: &'a str) -> Mail {
                        let _ = to;
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("pub async fn send_welcome < 'a >"));
        assert!(rendered.contains("self . welcome"));
        assert!(!rendered.contains("self . welcome :: < 'a >"));
    }

    #[test]
    fn skips_non_mail_returning_methods() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn helper(&self) -> String {
                        String::new()
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(!rendered.contains("send_helper"));
        assert!(!rendered.contains("deliver_later_helper"));
    }

    #[test]
    fn rejects_mutable_self_mail_templates() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    fn reset(&mut self, to: String) -> Mail {
                        panic!("template body is irrelevant to macro rendering test")
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(rendered.contains("template methods must use an `&self` receiver"));
    }

    #[test]
    fn ignores_async_non_mail_methods() {
        let out = mailer_macro(
            TokenStream::new(),
            quote! {
                impl AccountMailer {
                    async fn helper(&self) -> String {
                        String::new()
                    }
                }
            },
        );
        let rendered = out.to_string();
        assert!(!rendered.contains("send_helper"));
        assert!(!rendered.contains("deliver_later_helper"));
    }
}
