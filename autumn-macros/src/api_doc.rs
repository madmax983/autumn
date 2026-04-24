// OpenAPI appears many times in this module's docs as an acronym —
// silence clippy::doc_markdown for it locally. The other allows turn
// off pedantic style nits (`KeyValue::Foo` vs `Foo` inside the enum's
// own impl, `Option::map_or_else` vs `match`/`if let`) that would
// trade clarity for less-readable chained closure calls.
#![allow(
    clippy::doc_markdown,
    clippy::option_if_let_else,
    clippy::single_match_else,
    clippy::use_self
)]

//! `#[api_doc(...)]` attribute parsing for OpenAPI auto-generation.
//!
//! `#[api_doc]` is handled two ways:
//!
//! * As a **stored attribute** on route handlers (`#[get]`, `#[post]`, …):
//!   the route macro strips it from the function's attribute list,
//!   parses it here, and embeds the result in the generated `ApiDoc`
//!   struct.
//! * As a **standalone proc-macro** (see [`crate::api_doc`]) so Rust
//!   accepts the attribute on its own. That entry point is a no-op
//!   wrapper — the real work happens when a route macro is also
//!   applied, since routes are the only places metadata is collected.
//!
//! Supported forms:
//!
//! ```ignore
//! #[api_doc(summary = "Fetch a user", tag = "users")]
//! #[api_doc(description = "...", tags = ["users", "admin"], status = 201)]
//! #[api_doc(hidden)]
//! ```
//!
//! Unknown keys are a compile error, so typos surface at build time.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Expr, ExprLit, Ident, Lit, LitBool, LitStr, Token};

/// Parsed `#[api_doc(...)]` attribute arguments.
#[derive(Default)]
pub struct ApiDocAttr {
    pub summary: Option<LitStr>,
    pub description: Option<LitStr>,
    pub tags: Vec<LitStr>,
    pub operation_id: Option<LitStr>,
    pub status: Option<u16>,
    pub hidden: bool,
}

enum KeyValue {
    Summary(LitStr),
    Description(LitStr),
    Tag(LitStr),
    Tags(Vec<LitStr>),
    OperationId(LitStr),
    Status(u16),
    Hidden,
}

impl Parse for KeyValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        let key_str = key.to_string();

        if key_str == "hidden" {
            if input.peek(Token![=]) {
                let _eq: Token![=] = input.parse()?;
                let value: LitBool = input.parse()?;
                return Ok(if value.value {
                    KeyValue::Hidden
                } else {
                    // `hidden = false` is equivalent to the default (visible).
                    // Return a distinguishable marker via Tags(vec![]) so
                    // ApiDocAttr::merge does nothing — this keeps parsing
                    // symmetric with other bool forms.
                    KeyValue::Tags(Vec::new())
                });
            }
            return Ok(KeyValue::Hidden);
        }

        let _eq: Token![=] = input.parse()?;
        match key_str.as_str() {
            "summary" => Ok(KeyValue::Summary(input.parse()?)),
            "description" => Ok(KeyValue::Description(input.parse()?)),
            "tag" => Ok(KeyValue::Tag(input.parse()?)),
            "tags" => {
                // `tags = ["a", "b"]`
                let content;
                syn::bracketed!(content in input);
                let items =
                    syn::punctuated::Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                Ok(KeyValue::Tags(items.into_iter().collect()))
            }
            "operation_id" => Ok(KeyValue::OperationId(input.parse()?)),
            "status" => {
                let value: Expr = input.parse()?;
                let n = expect_u16(&value)?;
                Ok(KeyValue::Status(n))
            }
            other => Err(syn::Error::new(
                key.span(),
                format!(
                    "unknown key `{other}` in `#[api_doc(...)]`. \
                     Supported keys: summary, description, tag, tags, operation_id, status, hidden."
                ),
            )),
        }
    }
}

fn expect_u16(expr: &Expr) -> syn::Result<u16> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Int(int), ..
    }) = expr
    {
        int.base10_parse::<u16>()
    } else {
        Err(syn::Error::new_spanned(
            expr,
            "expected an integer HTTP status code (e.g. `status = 201`)",
        ))
    }
}

impl ApiDocAttr {
    fn merge(&mut self, kv: KeyValue) {
        match kv {
            KeyValue::Summary(v) => self.summary = Some(v),
            KeyValue::Description(v) => self.description = Some(v),
            KeyValue::Tag(v) => self.tags = vec![v],
            KeyValue::Tags(v) if !v.is_empty() => self.tags = v,
            KeyValue::Tags(_) => {}
            KeyValue::OperationId(v) => self.operation_id = Some(v),
            KeyValue::Status(n) => self.status = Some(n),
            KeyValue::Hidden => self.hidden = true,
        }
    }
}

impl Parse for ApiDocAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let items = syn::punctuated::Punctuated::<KeyValue, Token![,]>::parse_terminated(input)?;
        let mut out = ApiDocAttr::default();
        for kv in items {
            out.merge(kv);
        }
        Ok(out)
    }
}

/// Strip `#[api_doc(...)]` attributes from a handler's attribute list
/// and merge all of them into a single [`ApiDocAttr`].
///
/// Repeating the attribute is legal; later values override earlier ones
/// for scalar fields, and `tags = [..]` replaces the accumulated tags.
pub fn extract(attrs: &mut Vec<Attribute>) -> Result<ApiDocAttr, TokenStream> {
    let mut collected = ApiDocAttr::default();
    let mut error: Option<TokenStream> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident("api_doc") {
            return true;
        }
        // `#[api_doc]` with no arguments → mark visible with no overrides.
        let parsed: syn::Result<ApiDocAttr> = match &attr.meta {
            syn::Meta::Path(_) => Ok(ApiDocAttr::default()),
            syn::Meta::List(list) => syn::parse2(list.tokens.clone()),
            syn::Meta::NameValue(nv) => Err(syn::Error::new_spanned(
                nv,
                "expected `#[api_doc(...)]`, not `#[api_doc = ...]`",
            )),
        };
        match parsed {
            Ok(parsed) => {
                collected.absorb(parsed);
            }
            Err(err) => {
                if error.is_none() {
                    error = Some(err.to_compile_error());
                }
            }
        }
        false
    });

    if let Some(err) = error {
        return Err(err);
    }
    Ok(collected)
}

impl ApiDocAttr {
    fn absorb(&mut self, other: ApiDocAttr) {
        if other.summary.is_some() {
            self.summary = other.summary;
        }
        if other.description.is_some() {
            self.description = other.description;
        }
        if !other.tags.is_empty() {
            self.tags = other.tags;
        }
        if other.operation_id.is_some() {
            self.operation_id = other.operation_id;
        }
        if other.status.is_some() {
            self.status = other.status;
        }
        if other.hidden {
            self.hidden = true;
        }
    }

    /// Emit field initializers `summary: ..., description: ..., tags: ..., hidden: ...`
    /// for inclusion in an `ApiDoc { ... }` literal.
    ///
    /// `default_operation_id` is used when `operation_id` was not set on
    /// the attribute — typically the handler function's identifier.
    pub fn emit_ident_fields(&self, default_operation_id: &Ident) -> TokenStream {
        let summary = option_str(self.summary.as_ref());
        let description = option_str(self.description.as_ref());
        let tags = slice_str(&self.tags);
        let op_id = if let Some(id) = &self.operation_id {
            quote! { #id }
        } else {
            quote! { ::core::stringify!(#default_operation_id) }
        };
        let status = self.status.unwrap_or(200);
        let hidden = self.hidden;
        quote! {
            operation_id: #op_id,
            summary: #summary,
            description: #description,
            tags: #tags,
            success_status: #status,
            hidden: #hidden,
        }
    }
}

fn option_str(lit: Option<&LitStr>) -> TokenStream {
    match lit {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    }
}

fn slice_str(items: &[LitStr]) -> TokenStream {
    if items.is_empty() {
        quote! { &[] }
    } else {
        let literals: Vec<_> = items.iter().map(|s| quote! { #s }).collect();
        quote! { &[#(#literals),*] }
    }
}

// ──────────────────────────────────────────────────────────────────
// Route path + signature inspection helpers
// ──────────────────────────────────────────────────────────────────

/// Extract `{name}` path parameters from a route template.
///
/// Closing braces without an opening brace are ignored. Segments that
/// contain regex (`{id:[0-9]+}`) take only the name before the colon.
pub fn extract_path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end_rel) = bytes[i + 1..].iter().position(|b| *b == b'}') {
                let inner = &path[i + 1..i + 1 + end_rel];
                let name = inner.split(':').next().unwrap_or(inner).trim();
                if !name.is_empty() {
                    out.push(name.to_owned());
                }
                i += 1 + end_rel + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Emit a `&'static [&'static str]` literal for a list of owned strings.
pub fn emit_path_param_slice(params: &[String]) -> TokenStream {
    if params.is_empty() {
        quote! { &[] }
    } else {
        let literals: Vec<_> = params
            .iter()
            .map(|p| LitStr::new(p, Span::call_site()))
            .collect();
        quote! { &[#(#literals),*] }
    }
}

/// Inspect the handler's parameter list for `Json<T>` request bodies.
///
/// Returns a `Some(tokens)` producing a `SchemaEntry` initializer for the
/// first JSON extractor seen, or `None` if the handler has no JSON body.
pub fn infer_request_body(input_fn: &syn::ItemFn) -> Option<TokenStream> {
    for arg in &input_fn.sig.inputs {
        let syn::FnArg::Typed(pat) = arg else {
            continue;
        };
        if let Some(inner) = unwrap_single_generic(&pat.ty, "Json") {
            return Some(schema_entry_for_type(&inner));
        }
    }
    None
}

/// Inspect the handler's return type for `Json<T>` to infer the success
/// response body. Handles several common Axum return-type patterns:
///
/// * `Json<T>` — plain JSON body
/// * `Result<Json<T>, _>` / `AutumnResult<Json<T>>` — fallible JSON
/// * `(StatusCode, Json<T>)` — JSON with a custom status code
/// * `Result<(StatusCode, Json<T>), _>` — the two combined
pub fn infer_response_body(input_fn: &syn::ItemFn) -> Option<TokenStream> {
    let syn::ReturnType::Type(_, ty) = &input_fn.sig.output else {
        return None;
    };
    let ty = unwrap_result_ok(ty).unwrap_or_else(|| (**ty).clone());
    find_json_in_type(&ty).map(|inner| schema_entry_for_type(&inner))
}

/// Look for `Json<T>` either directly or inside a tuple element.
///
/// Axum handlers often return tuples like `(StatusCode, Json<T>)` or
/// `([(HeaderName, _); N], Json<T>)` to attach status codes or
/// headers. We scan each tuple element so the generated schema still
/// reflects the JSON body.
fn find_json_in_type(ty: &syn::Type) -> Option<syn::Type> {
    if let Some(inner) = unwrap_single_generic(ty, "Json") {
        return Some(inner);
    }
    if let syn::Type::Tuple(tup) = ty {
        for elem in &tup.elems {
            if let Some(inner) = unwrap_single_generic(elem, "Json") {
                return Some(inner);
            }
        }
    }
    None
}

/// Peel a single layer of `Result<T, _>` / `AutumnResult<T>` so we can
/// inspect the `Ok` variant for a `Json<...>` wrapper.
fn unwrap_result_ok(ty: &syn::Type) -> Option<syn::Type> {
    let path = match ty {
        syn::Type::Path(p) => &p.path,
        _ => return None,
    };
    let last = path.segments.last()?;
    let name = last.ident.to_string();
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match name.as_str() {
        "Result" => args.args.iter().find_map(|arg| match arg {
            syn::GenericArgument::Type(t) => Some(t.clone()),
            _ => None,
        }),
        "AutumnResult" => args.args.iter().find_map(|arg| match arg {
            syn::GenericArgument::Type(t) => Some(t.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// If `ty` is `Name<Inner>` (single generic argument), return `Inner`.
/// The outermost segment of `ty`'s path must match `wrapper`.
fn unwrap_single_generic(ty: &syn::Type, wrapper: &str) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != wrapper {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    })
}

/// Emit a `::autumn_web::openapi::SchemaEntry` initializer for a type.
///
/// The generated code dispatches at compile time to a const-fn-like
/// branching expression that yields either:
/// * a primitive schema when the type matches a known scalar, or
/// * a `Ref` referencing the type's [`OpenApiSchema::schema_name()`]
///   when the type is any other named path, or
/// * a fallback "object"-typed name when we cannot identify the type.
fn schema_entry_for_type(ty: &syn::Type) -> TokenStream {
    // Lift primitive detection out of the proc macro so the user's path
    // like `crate::models::User` isn't re-parsed at compile time. We emit
    // a const-dispatched expression using type-id-less pattern matching
    // via `match` on a generated helper; the simpler approach here is to
    // always emit a `Ref` with the type's last-segment name. This keeps
    // the macro output short while still permitting users to register
    // their types via `OpenApiSchema`.
    let name = last_segment_name(ty).unwrap_or_else(|| "Schema".to_owned());
    let name_lit = LitStr::new(&name, Span::call_site());
    let primitive = primitive_json_type(&name);
    if let Some(json_type) = primitive {
        let json_lit = LitStr::new(json_type, Span::call_site());
        quote! {
            ::autumn_web::openapi::SchemaEntry {
                name: #name_lit,
                kind: ::autumn_web::openapi::SchemaKind::Primitive(#json_lit),
            }
        }
    } else {
        quote! {
            ::autumn_web::openapi::SchemaEntry {
                name: #name_lit,
                kind: ::autumn_web::openapi::SchemaKind::Ref,
            }
        }
    }
}

/// Map a short Rust primitive name to its JSON-schema `type` keyword.
fn primitive_json_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "String" | "str" => "string",
        "bool" => "boolean",
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize" => {
            "integer"
        }
        "f32" | "f64" => "number",
        _ => return None,
    })
}

/// Return the final identifier in a type's path (e.g. `foo::Bar` → `"Bar"`).
fn last_segment_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        syn::Type::Reference(r) => last_segment_name(&r.elem),
        _ => None,
    }
}

/// Convenience wrapper: emit an `Option<SchemaEntry>` expression.
pub fn schema_option(expr: Option<TokenStream>) -> TokenStream {
    match expr {
        Some(e) => quote! { ::core::option::Option::Some(#e) },
        None => quote! { ::core::option::Option::None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_path_params_handles_single() {
        assert_eq!(extract_path_params("/users/{id}"), vec!["id".to_owned()]);
    }

    #[test]
    fn extract_path_params_handles_multiple() {
        assert_eq!(
            extract_path_params("/posts/{year}/{slug}"),
            vec!["year".to_owned(), "slug".to_owned()]
        );
    }

    #[test]
    fn extract_path_params_handles_regex_prefix() {
        assert_eq!(
            extract_path_params("/users/{id:[0-9]+}"),
            vec!["id".to_owned()]
        );
    }

    #[test]
    fn extract_path_params_returns_empty_for_static() {
        assert!(extract_path_params("/hello").is_empty());
        assert!(extract_path_params("/").is_empty());
    }

    #[test]
    fn extract_path_params_ignores_unclosed_braces() {
        assert!(extract_path_params("/oops/{broken").is_empty());
    }

    #[test]
    fn primitive_json_type_matches_common() {
        assert_eq!(primitive_json_type("String"), Some("string"));
        assert_eq!(primitive_json_type("i64"), Some("integer"));
        assert_eq!(primitive_json_type("bool"), Some("boolean"));
        assert_eq!(primitive_json_type("Foo"), None);
    }
}
