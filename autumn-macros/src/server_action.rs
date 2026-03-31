use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, ReturnType, Type};

#[allow(clippy::too_many_lines)]
pub fn server_macro(item: TokenStream) -> TokenStream {
    let input: ItemFn = match syn::parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    if input.sig.asyncness.is_none() {
        return syn::Error::new_spanned(input.sig.fn_token, "#[server] requires async fn")
            .to_compile_error();
    }

    if input.sig.inputs.len() != 1 {
        return syn::Error::new_spanned(
            &input.sig.ident,
            "#[server] requires exactly one input argument",
        )
        .to_compile_error();
    }

    if !input.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.sig.generics,
            "#[server] does not support generic functions yet",
        )
        .to_compile_error();
    }

    let input_arg = match input.sig.inputs.first().expect("validated len") {
        FnArg::Typed(typed) => typed,
        FnArg::Receiver(recv) => {
            return syn::Error::new_spanned(recv, "#[server] does not support methods")
                .to_compile_error();
        }
    };

    let input_type = (*input_arg.ty).clone();
    let input_binding = match input_arg.pat.as_ref() {
        syn::Pat::Ident(ident) => ident.ident.clone(),
        other => {
            return syn::Error::new_spanned(
                other,
                "#[server] input argument pattern must be an identifier",
            )
            .to_compile_error();
        }
    };
    let output_type = match extract_output_type(&input.sig.output) {
        Ok(output) => output,
        Err(err) => return err.to_compile_error(),
    };

    let attrs = &input.attrs;
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
        .collect();
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;
    let name = &input.sig.ident;

    let route_path = format!("/_autumn/actions/{name}");
    let action_handler = format_ident!("__autumn_action_handler_{name}");
    let action_route = format_ident!("__autumn_action_route_{name}");
    let action_meta = format_ident!("__autumn_action_meta_{name}");

    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #(#attrs)*
        #vis #sig #block

        #[cfg(target_arch = "wasm32")]
        #(#cfg_attrs)*
        #vis async fn #name(#input_binding: #input_type) -> ::autumn_web::AutumnResult<#output_type> {
            ::autumn_web::wasm::post_json::<#input_type, #output_type>(#route_path, &#input_binding)
                .await
                .map_err(|error| {
                    ::autumn_web::AutumnError::internal_msg(
                        format!("server action `{}` failed: {error}", stringify!(#name)),
                    )
                })
        }

        #[cfg(not(target_arch = "wasm32"))]
        #(#cfg_attrs)*
        #[doc(hidden)]
        pub async fn #action_handler(
            ::autumn_web::extract::Json(input): ::autumn_web::extract::Json<#input_type>,
        ) -> ::autumn_web::AutumnResult<::autumn_web::extract::Json<#output_type>> {
            #name(input).await.map(::autumn_web::extract::Json)
        }

        #[cfg(not(target_arch = "wasm32"))]
        #(#cfg_attrs)*
        #[doc(hidden)]
        pub fn #action_route() -> ::autumn_web::route::Route {
            ::autumn_web::route::Route {
                method: ::autumn_web::reexports::http::Method::POST,
                path: #route_path,
                handler: ::autumn_web::reexports::axum::routing::post(#action_handler),
                name: stringify!(#name),
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        #(#cfg_attrs)*
        #[doc(hidden)]
        pub fn #action_meta() -> ::autumn_web::wasm::ActionMeta {
            ::autumn_web::wasm::ActionMeta {
                name: stringify!(#name),
                path: #route_path,
                route: #action_route,
            }
        }

        #[cfg(target_arch = "wasm32")]
        #(#cfg_attrs)*
        #[doc(hidden)]
        pub fn #action_meta() -> ::autumn_web::wasm::ActionMeta {
            ::autumn_web::wasm::ActionMeta {
                name: stringify!(#name),
                path: #route_path,
                route: ::autumn_web::wasm::noop_action_route,
            }
        }
    }
}

fn extract_output_type(return_type: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = return_type else {
        return Err(syn::Error::new_spanned(
            return_type,
            "#[server] requires an explicit return type AutumnResult<T>",
        ));
    };

    let Type::Path(type_path) = &**ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "#[server] return type must be AutumnResult<T>",
        ));
    };

    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new_spanned(
            ty,
            "#[server] return type must be AutumnResult<T>",
        ));
    };

    if segment.ident != "AutumnResult" {
        return Err(syn::Error::new_spanned(
            ty,
            "#[server] return type must be AutumnResult<T>",
        ));
    }

    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            ty,
            "#[server] return type must be AutumnResult<T>",
        ));
    };

    let Some(syn::GenericArgument::Type(output)) = args.args.first() else {
        return Err(syn::Error::new_spanned(
            ty,
            "#[server] return type must be AutumnResult<T>",
        ));
    };

    Ok(output.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn rejects_non_async_functions() {
        let out = server_macro(quote! {
            fn rename(input: RenameTodo) -> AutumnResult<TodoView> { todo!() }
        });

        assert!(out.to_string().contains("requires async fn"));
    }

    #[test]
    fn rejects_functions_without_single_input() {
        let out = server_macro(quote! {
            async fn rename(a: RenameTodo, b: RenameTodo) -> AutumnResult<TodoView> { todo!() }
        });

        assert!(out.to_string().contains("exactly one input argument"));
    }

    #[test]
    fn rejects_non_autumn_result_return_type() {
        let out = server_macro(quote! {
            async fn rename(input: RenameTodo) -> TodoView { todo!() }
        });

        assert!(out.to_string().contains("AutumnResult<T>"));
    }

    #[test]
    fn rejects_generic_functions() {
        let out = server_macro(quote! {
            async fn rename<T>(input: RenameTodo<T>) -> AutumnResult<TodoView> { todo!() }
        });

        assert!(
            out.to_string()
                .contains("does not support generic functions")
        );
    }

    #[test]
    fn expands_server_action_helpers() {
        let out = server_macro(quote! {
            async fn rename_todo(input: RenameTodo) -> AutumnResult<TodoView> {
                todo!()
            }
        })
        .to_string();

        assert!(out.contains("__autumn_action_meta_rename_todo"));
        assert!(out.contains("__autumn_action_route_rename_todo"));
        assert!(out.contains("/_autumn/actions/rename_todo"));
    }

    #[test]
    fn propagates_cfg_attributes_to_generated_companions() {
        let out = server_macro(quote! {
            #[cfg(feature = "experimental")]
            async fn rename_todo(input: RenameTodo) -> AutumnResult<TodoView> {
                todo!()
            }
        })
        .to_string();

        let cfg_count = out.matches("# [cfg (feature = \"experimental\")]").count();
        assert!(cfg_count >= 4);
    }
}
