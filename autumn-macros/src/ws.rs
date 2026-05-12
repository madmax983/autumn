//! WebSocket route macro implementation.
//!
//! Generates a WebSocket upgrade handler from a user function that
//! follows the two-function pattern: the outer function runs at
//! upgrade time (with access to extractors) and returns a closure
//! implementing `WsHandler` that handles the live socket.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parse;

/// Check if a type pattern looks like `AppState` (bare identifier).
fn is_app_state_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && type_path.qself.is_none()
    {
        let segments: Vec<_> = type_path.path.segments.iter().collect();
        // Match `AppState` or `autumn_web::AppState` etc.
        if let Some(last) = segments.last() {
            return last.ident == "AppState" && last.arguments.is_none();
        }
    }
    false
}

/// Implementation of the `#[ws("/path")]` attribute macro.
///
/// Given a user function like:
///
/// ```ignore
/// #[ws("/echo")]
/// async fn echo(state: AppState) -> impl WsHandler {
///     |mut socket: WebSocket| async move { /* ... */ }
/// }
/// ```
///
/// Generates:
///
/// 1. The user's function (unchanged)
/// 2. A `__autumn_ws_upgrade_echo` handler that extracts `WebSocketUpgrade`
///    + `State<AppState>`, calls the user function, and upgrades.
/// 3. A `__autumn_route_info_echo` companion returning a `Route` (GET)
///    so `routes![]` works seamlessly.
///
/// The user's function parameters are treated as follows:
/// - `AppState` parameters receive the extracted app state directly
/// - All other parameters become Axum extractors on the upgrade handler
pub fn ws_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = match parse::parse_route_path(attr) {
        Ok(p) => p,
        Err(err) => return err,
    };

    let input_fn = match parse::parse_async_handler(item) {
        Ok(f) => f,
        Err(err) => return err,
    };

    let fn_name = &input_fn.sig.ident;
    let vis = &input_fn.vis;
    let upgrade_name = format_ident!("__autumn_ws_upgrade_{}", fn_name);
    let route_info_name = format_ident!("__autumn_route_info_{}", fn_name);

    // Separate user params into AppState params (supplied from extracted state)
    // and extractor params (become Axum extractors on the upgrade handler).
    let mut extractor_params = Vec::new();
    let mut call_args = Vec::new();

    for arg in &input_fn.sig.inputs {
        if let syn::FnArg::Typed(pat_type) = arg {
            if is_app_state_type(&pat_type.ty) {
                // AppState param — supply from our extracted state
                call_args.push(quote! { __autumn_state.clone() });
            } else {
                // Regular extractor — add to upgrade handler params
                let pat = &pat_type.pat;
                extractor_params.push(arg.clone());
                call_args.push(quote! { #pat });
            }
        }
    }

    let upgrade_handler = if extractor_params.is_empty() {
        quote! {
            #[doc(hidden)]
            #vis async fn #upgrade_name(
                __autumn_ws: ::autumn_web::ws::WebSocketUpgrade,
                ::autumn_web::reexports::axum::extract::State(__autumn_state): ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>,
            ) -> impl ::autumn_web::reexports::axum::response::IntoResponse {
                let __autumn_shutdown = __autumn_state.shutdown_token();
                let handler = #fn_name(#(#call_args),*).await;
                __autumn_ws.on_upgrade(move |socket| async move {
                    ::autumn_web::ws::WsHandler::handle(handler, socket, __autumn_shutdown).await;
                })
            }
        }
    } else {
        quote! {
            #[doc(hidden)]
            #vis async fn #upgrade_name(
                __autumn_ws: ::autumn_web::ws::WebSocketUpgrade,
                ::autumn_web::reexports::axum::extract::State(__autumn_state): ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>,
                #(#extractor_params),*
            ) -> impl ::autumn_web::reexports::axum::response::IntoResponse {
                let __autumn_shutdown = __autumn_state.shutdown_token();
                let handler = #fn_name(#(#call_args),*).await;
                __autumn_ws.on_upgrade(move |socket| async move {
                    ::autumn_web::ws::WsHandler::handle(handler, socket, __autumn_shutdown).await;
                })
            }
        }
    };

    let path_value = path.value();
    let path_params = crate::api_doc::extract_path_params(&path_value);
    let path_params_tokens = crate::api_doc::emit_path_param_slice(&path_params);

    quote! {
        #input_fn

        #upgrade_handler

        #[doc(hidden)]
        #vis fn #route_info_name() -> ::autumn_web::Route {
            ::autumn_web::Route {
                method: ::autumn_web::reexports::http::Method::from_bytes(b"WS")
                    .expect("WS is a valid method token"),
                path: #path,
                handler: ::autumn_web::reexports::axum::routing::get(#upgrade_name),
                name: ::core::stringify!(#fn_name),
                // WebSocket upgrades don't have a meaningful JSON body, so
                // they are excluded from the generated OpenAPI spec by
                // default. Users wanting to document them can add their
                // own entries via `OpenApiConfig::register_schema`.
                api_doc: ::autumn_web::openapi::ApiDoc {
                    method: "GET",
                    path: #path,
                    operation_id: ::core::stringify!(#fn_name),
                    summary: ::core::option::Option::None,
                    description: ::core::option::Option::None,
                    tags: &[],
                    path_params: #path_params_tokens,
                    request_body: ::core::option::Option::None,
                    response: ::core::option::Option::None,
                    success_status: 101,
                    hidden: true,
                    query_schema: ::core::option::Option::None,
                    secured: false,
                    required_roles: &[],
                    register_schemas: ::core::option::Option::None,
                },
                repository: ::core::option::Option::None,
            }
        }
    }
}
