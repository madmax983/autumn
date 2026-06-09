//! `#[model]` attribute macro implementation.
//!
//! Generates four types from a single struct:
//! - The query type (original struct) with `Queryable`, `Selectable`
//! - A `NewX` insert type with `Insertable` (ID fields excluded)
//! - An `UpdateX` patch type with `Default` (ID fields excluded, all `Patch<T>`)
//! - A `XField` enum with one variant per mutable field (for audit/CDC payloads)
//!
//! Also generates on `UpdateDraft<Model>`:
//! - `from_patch(current, patch)` — merges a `Patch`-based update into a draft
//! - Per-field `DraftField` accessor methods for inspecting/overriding changes
//!
//! Recognises `#[id]`, `#[indexed]`, and `#[validate(...)]` field attributes.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{DeriveInput, Field, LitStr};

/// Process `#[model]` or `#[model(table = "...")]` attribute arguments.
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

/// Check if a field has the `#[id]` attribute.
fn has_attr(field: &Field, name: &str) -> bool {
    field.attrs.iter().any(|a| a.path().is_ident(name))
}

/// Extract `#[validate(...)]` attributes from a field (verbatim pass-through).
fn validate_attrs(field: &Field) -> Vec<&syn::Attribute> {
    field
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("validate"))
        .collect()
}

/// Filter out framework-specific attributes (`#[id]`, `#[indexed]`, `#[validate]`,
/// `#[default]`, `#[factory_assoc]`, `#[lock_version]`, `#[searchable]`,
/// `#[state_machine]`) that shouldn't be on the query struct
/// (they'd confuse Diesel derives).
fn user_attrs(field: &Field) -> Vec<&syn::Attribute> {
    field
        .attrs
        .iter()
        .filter(|a| {
            !a.path().is_ident("id")
                && !a.path().is_ident("indexed")
                && !a.path().is_ident("validate")
                && !a.path().is_ident("default")
                && !a.path().is_ident("factory_assoc")
                && !a.path().is_ident("lock_version")
                && !a.path().is_ident("searchable")
                && !a.path().is_ident("encrypted")
                && !a.path().is_ident("state_machine")
        })
        .collect()
}

/// Encryption mode requested by an `#[encrypted]` field attribute.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EncryptedMode {
    /// Not an encrypted field.
    None,
    /// `#[encrypted]` — randomized AEAD (default; no equality lookups).
    Randomized,
    /// `#[encrypted(deterministic)]` — stable ciphertext; supports equality
    /// lookups, at the cost of leaking plaintext equality through ciphertext.
    Deterministic,
}

/// Parsed `#[encrypted(...)]` field specification.
#[derive(Clone, Copy)]
struct EncryptedSpec {
    mode: EncryptedMode,
    /// `admin_visible` — render decrypted plaintext in admin views (the admin
    /// surface itself is authorization-gated; #496). Default: redacted.
    admin_visible: bool,
    /// `versioned_ciphertext` — store encrypted before/after ciphertext in record
    /// version history instead of the default "changed (encrypted)" marker.
    versioned_ciphertext: bool,
}

impl EncryptedSpec {
    const NONE: Self = Self {
        mode: EncryptedMode::None,
        admin_visible: false,
        versioned_ciphertext: false,
    };
    fn is_encrypted(self) -> bool {
        self.mode != EncryptedMode::None
    }
}

/// Parse an `#[encrypted]` / `#[encrypted(deterministic, admin_visible, ...)]`
/// field attribute.
fn parse_field_encrypted(field: &syn::Field) -> syn::Result<EncryptedSpec> {
    for attr in &field.attrs {
        if !attr.path().is_ident("encrypted") {
            continue;
        }
        // `#[encrypted]` (bare path) -> randomized, no opt-ins.
        if matches!(attr.meta, syn::Meta::Path(_)) {
            return Ok(EncryptedSpec {
                mode: EncryptedMode::Randomized,
                ..EncryptedSpec::NONE
            });
        }
        let mut spec = EncryptedSpec {
            mode: EncryptedMode::Randomized,
            ..EncryptedSpec::NONE
        };
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("deterministic") {
                spec.mode = EncryptedMode::Deterministic;
                Ok(())
            } else if meta.path.is_ident("randomized") {
                spec.mode = EncryptedMode::Randomized;
                Ok(())
            } else if meta.path.is_ident("admin_visible") {
                spec.admin_visible = true;
                Ok(())
            } else if meta.path.is_ident("versioned_ciphertext") {
                spec.versioned_ciphertext = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unsupported `#[encrypted]` option; expected one of \
                     `deterministic`, `randomized`, `admin_visible`, `versioned_ciphertext`",
                ))
            }
        })?;
        return Ok(spec);
    }
    Ok(EncryptedSpec::NONE)
}

/// Convenience: just the mode (used by the diesel-wrapper routing).
fn parse_field_encrypted_mode(field: &syn::Field) -> syn::Result<EncryptedMode> {
    Ok(parse_field_encrypted(field)?.mode)
}

/// Build a manual `Debug` impl that redacts encrypted fields, so plaintext
/// (held in memory as a `String` for ergonomics) never appears in `Debug`
/// output, panic backtraces, or framework error messages. The development-only
/// escape hatch (`encryption::set_debug_plaintext`) opts back into plaintext.
fn redacting_debug_impl(
    struct_name: &syn::Ident,
    field_idents: &[&syn::Ident],
    encrypted_names: &[&str],
) -> TokenStream {
    let stmts = field_idents.iter().map(|ident| {
        let nm = ident.to_string();
        if encrypted_names.contains(&nm.as_str()) {
            quote! {
                if ::autumn_web::encryption::debug_plaintext_enabled() {
                    s.field(#nm, &self.#ident);
                } else {
                    s.field(#nm, &::core::format_args!("<encrypted>"));
                }
            }
        } else {
            quote! { s.field(#nm, &self.#ident); }
        }
    });
    quote! {
        impl ::core::fmt::Debug for #struct_name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                let mut s = f.debug_struct(stringify!(#struct_name));
                #(#stmts)*
                s.finish()
            }
        }
    }
}

/// The `serialize_as`/`deserialize_as` wrapper path for an encrypted mode.
fn encrypted_wrapper_path(mode: EncryptedMode) -> Option<TokenStream> {
    match mode {
        EncryptedMode::None => None,
        EncryptedMode::Randomized => Some(quote! { ::autumn_web::encryption::RandomizedText }),
        EncryptedMode::Deterministic => {
            Some(quote! { ::autumn_web::encryption::DeterministicText })
        }
    }
}

/// Validate that `#[encrypted]` is only applied to a plain `String` field.
///
/// v1 supports non-null `String` columns (the realistic targets: tokens, SSNs,
/// emails). `Option<String>` and other types are rejected with a clear message.
fn validate_encrypted_field(field: &syn::Field) -> syn::Result<()> {
    if !parse_field_encrypted(field)?.is_encrypted() {
        return Ok(());
    }
    let is_string = matches!(&field.ty, syn::Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "String"));
    if !is_string {
        return Err(syn::Error::new_spanned(
            &field.ty,
            "`#[encrypted]` is only supported on non-null `String` fields in v1 \
             (encrypt before storing structured/optional data)",
        ));
    }
    // `#[encrypted]` columns must flow through the `serialize_as` wrapper on
    // insert. Fields excluded from the insert (`#[id]`, `#[default]`,
    // `#[lock_version]`) would instead get a raw database value, which the
    // decrypting reader then rejects as a malformed envelope. Reject the combo.
    if has_attr(field, "default") || has_attr(field, "lock_version") || has_attr(field, "id") {
        return Err(syn::Error::new_spanned(
            field,
            "`#[encrypted]` cannot be combined with `#[default]`, `#[lock_version]`, \
             or `#[id]`: those fields bypass the insert path, so the column would \
             store an unencrypted value. Set the encrypted value explicitly on insert.",
        ));
    }
    // Full-text search builds the stored `search_vector` from the database column
    // value, which for an encrypted field is ciphertext. Indexing/querying that
    // would match envelope tokens, not the plaintext, so the repository's `search`
    // would silently miss encrypted content. Reject the combination.
    if has_attr(field, "searchable") {
        return Err(syn::Error::new_spanned(
            field,
            "`#[encrypted]` cannot be combined with `#[searchable]`: full-text search \
             indexes the stored column, which holds ciphertext, so plaintext searches \
             would never match. Remove `#[searchable]` from the encrypted field (keep a \
             separate non-encrypted column if you need to search).",
        ));
    }
    // The encrypted column is registered under its Rust field name, which the
    // log-scrub / version-history / admin compositions match against the
    // serde-serialized key. A `#[serde(rename)]` would desync those, leaking the
    // renamed plaintext (e.g. into version history). Reject it in v1.
    if field_has_serde_rename(field) {
        return Err(syn::Error::new_spanned(
            field,
            "`#[encrypted]` fields cannot use `#[serde(rename = ...)]` in v1: the \
             column is registered under its Rust name, which must match the \
             serialized key used by version history / log scrubbing / admin redaction.",
        ));
    }
    Ok(())
}

/// Whether any attribute is a struct-level `#[serde(rename_all = "...")]`.
fn attrs_have_serde_rename_all(attrs: &[syn::Attribute]) -> bool {
    let mut found = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                found = true;
            }
            if let Ok(value) = meta.value() {
                let _: syn::Result<syn::Lit> = value.parse();
            }
            Ok(())
        });
    }
    found
}

/// Whether a field carries a `#[serde(rename = "...")]` (which would desync the
/// encrypted-column registry from the serialized key).
fn field_has_serde_rename(field: &syn::Field) -> bool {
    let mut renamed = false;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                renamed = true;
            }
            // Consume any `= value` so sibling metas keep parsing.
            if let Ok(value) = meta.value() {
                let _: syn::Result<syn::Lit> = value.parse();
            }
            Ok(())
        });
    }
    renamed
}

/// Parse the struct-level language dictionary configuration from `#[searchable(language = "...")]`
fn parse_model_searchable_lang(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("searchable") {
            if matches!(attr.meta, syn::Meta::Path(_)) {
                return Ok(Some("simple".to_string()));
            }
            let mut lang = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("language") {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    lang = Some(value.value());
                    Ok(())
                } else {
                    Err(meta.error("unsupported searchable attribute"))
                }
            })?;
            return Ok(Some(lang.unwrap_or_else(|| "simple".to_string())));
        }
    }
    Ok(None)
}

enum FieldSearchable {
    NotSearchable,
    SearchableDefault,
    SearchableWithWeight(String),
}

/// Parse the field-level weight from `#[searchable(weight = "...")]`
fn parse_field_searchable_weight(field: &syn::Field) -> syn::Result<FieldSearchable> {
    for attr in &field.attrs {
        if attr.path().is_ident("searchable") {
            if matches!(attr.meta, syn::Meta::Path(_)) {
                return Ok(FieldSearchable::SearchableDefault);
            }
            let mut weight = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("weight") {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    weight = Some(value.value());
                    Ok(())
                } else {
                    Err(meta.error("unsupported field searchable attribute"))
                }
            })?;
            return Ok(weight.map_or(
                FieldSearchable::SearchableDefault,
                FieldSearchable::SearchableWithWeight,
            ));
        }
    }
    Ok(FieldSearchable::NotSearchable)
}

#[derive(Clone, Copy)]
enum SerdeAdapterMode {
    Serialize,
    Deserialize,
}

#[derive(Default)]
struct SerdeAdapterAttrs {
    with: Option<LitStr>,
    serialize_with: Option<LitStr>,
    deserialize_with: Option<LitStr>,
}

fn serde_adapter_attrs(field: &Field) -> SerdeAdapterAttrs {
    let mut adapters = SerdeAdapterAttrs::default();
    for attr in field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("serde"))
    {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("with") {
                adapters.with = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("serialize_with") {
                adapters.serialize_with = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("deserialize_with") {
                adapters.deserialize_with = Some(meta.value()?.parse()?);
            }
            Ok(())
        });
    }
    adapters
}

fn hook_serde_adapter_attrs(field: &Field, mode: SerdeAdapterMode) -> Vec<TokenStream> {
    let adapters = serde_adapter_attrs(field);
    let mut entries = Vec::new();
    if let Some(with) = adapters.with {
        entries.push(quote! { with = #with });
    }
    match mode {
        SerdeAdapterMode::Serialize => {
            if let Some(serialize_with) = adapters.serialize_with {
                entries.push(quote! { serialize_with = #serialize_with });
            }
        }
        SerdeAdapterMode::Deserialize => {
            if let Some(deserialize_with) = adapters.deserialize_with {
                entries.push(quote! { deserialize_with = #deserialize_with });
            }
        }
    }

    if entries.is_empty() {
        Vec::new()
    } else {
        vec![quote! { #[serde(#(#entries),*)] }]
    }
}

fn has_hook_serde_adapter(field: &Field, mode: SerdeAdapterMode) -> bool {
    let adapters = serde_adapter_attrs(field);
    adapters.with.is_some()
        || match mode {
            SerdeAdapterMode::Serialize => adapters.serialize_with.is_some(),
            SerdeAdapterMode::Deserialize => adapters.deserialize_with.is_some(),
        }
}

enum SerdeDefaultKind {
    Default,
    Path(syn::Path),
}

fn serde_default_kind(field: &Field) -> Option<SerdeDefaultKind> {
    let mut default = None;
    for attr in field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("serde"))
    {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                if meta.input.peek(syn::Token![=]) {
                    let value: LitStr = meta.value()?.parse()?;
                    if let Ok(path) = value.parse::<syn::Path>() {
                        default = Some(SerdeDefaultKind::Path(path));
                    }
                } else {
                    default = Some(SerdeDefaultKind::Default);
                }
            }
            Ok(())
        });
    }
    default
}

fn commit_hook_missing_field_default_expr(field: &Field) -> Option<TokenStream> {
    match serde_default_kind(field) {
        Some(SerdeDefaultKind::Default) => Some(quote! { ::core::default::Default::default() }),
        Some(SerdeDefaultKind::Path(path)) => Some(quote! { #path() }),
        None if is_option_type(&field.ty) => Some(quote! { ::core::option::Option::None }),
        None => None,
    }
}

/// Extract the associated model type from `#[factory_assoc(TypeName)]` if present.
///
/// Returns `Some(Ident)` for the associated type, or `None` if the attribute is absent.
/// Panics if `factory_assoc` is present but fails to parse — callers should run
/// `validate_factory_assoc_attrs` first to surface a proper compile error.
fn factory_assoc_type(field: &Field) -> Option<syn::Ident> {
    for attr in &field.attrs {
        if attr.path().is_ident("factory_assoc")
            && let Ok(ident) = attr.parse_args::<syn::Ident>()
        {
            return Some(ident);
        }
    }
    None
}

/// Validate that every `#[factory_assoc(...)]` attribute contains a valid Ident.
///
/// Returns a compile error token stream on the first malformed attribute so the
/// user gets a clear diagnostic instead of silent fallback-to-normal-field behavior.
fn validate_factory_assoc_attrs(fields: &[&Field]) -> Option<TokenStream> {
    for field in fields {
        for attr in &field.attrs {
            if attr.path().is_ident("factory_assoc") {
                // Reject unparseable attribute argument.
                if let Err(err) = attr.parse_args::<syn::Ident>() {
                    return Some(err.to_compile_error());
                }
                // Reject Option<T> fields — the factory uses Option<T> itself to
                // represent "not yet set vs. explicit value", so Option<Option<T>>
                // would be generated, leading to an arm-type mismatch in create().
                if is_option_type(&field.ty) {
                    return Some(
                        syn::Error::new_spanned(
                            attr,
                            "#[factory_assoc] cannot be applied to an Option<T> field; \
                             factory_assoc is designed for non-nullable FK fields (e.g. i64). \
                             Use a plain field setter to supply a nullable association.",
                        )
                        .to_compile_error(),
                    );
                }
            }
        }
    }
    None
}

/// True if a field has `#[id]`, `#[default]`, or `#[lock_version]` — all
/// three are excluded from the `NewX` insert type.
///
/// `#[lock_version]` fields are excluded because the DB column must carry a
/// `DEFAULT 0` constraint; the initial version is always zero and is never
/// supplied by the caller on insert.
fn excluded_from_new(field: &Field) -> bool {
    has_attr(field, "id") || has_attr(field, "default") || has_attr(field, "lock_version")
}

/// Convert a `snake_case` identifier to `PascalCase`.
fn pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().to_string() + &chars.collect::<String>()
            })
        })
        .collect()
}

/// Check whether a type is `Option<...>`.
fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(tp) = ty {
        tp.path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Option")
    } else {
        false
    }
}

/// Return the final path segment name of a type (e.g. `foo::Bar` → `"Bar"`).
fn type_name_str(ty: &syn::Type) -> String {
    crate::api_doc::last_segment_name(ty).unwrap_or_else(|| "unknown".to_owned())
}

/// Emit a `TokenStream` that evaluates (at runtime) to a `serde_json::Value`
/// representing the JSON Schema for the given Rust type.
///
/// Handles `Option<T>` (nullable), `Vec<T>` (array), primitives (`String`,
/// `i64`, etc.), and everything else as a `$ref` to a component schema.
fn emit_json_schema_tokens(ty: &syn::Type) -> TokenStream {
    // Option<T> → OpenAPI 3.1 nullable: oneOf [{T-schema}, {type:null}]
    if let Some(inner) = crate::api_doc::unwrap_single_generic(ty, "Option") {
        let inner_tokens = emit_json_schema_tokens(&inner);
        return quote! {{
            let __inner = #inner_tokens;
            ::autumn_web::reexports::serde_json::json!({ "oneOf": [__inner, { "type": "null" }] })
        }};
    }

    // Vec<T> → {"type": "array", "items": <T-schema>}
    if let Some(inner) = crate::api_doc::unwrap_single_generic(ty, "Vec") {
        let inner_tokens = emit_json_schema_tokens(&inner);
        return quote! {{
            let __items = #inner_tokens;
            ::autumn_web::reexports::serde_json::json!({ "type": "array", "items": __items })
        }};
    }

    let name = type_name_str(ty);
    crate::api_doc::primitive_json_type(&name).map_or_else(
        || {
            let ref_path = format!("#/components/schemas/{name}");
            quote! { ::autumn_web::reexports::serde_json::json!({ "$ref": #ref_path }) }
        },
        |json_type| {
            quote! { ::autumn_web::reexports::serde_json::json!({ "type": #json_type }) }
        },
    )
}

/// Emit the body of `OpenApiSchema::schema()` for a list of fields.
///
/// `all_optional` is `true` for `UpdateX` structs where every field is
/// conceptually optional (backed by `Patch<T>`).
fn emit_schema_fn_body(fields: &[&&Field], all_optional: bool) -> TokenStream {
    emit_schema_fn_body_ext(fields, all_optional, &[])
}

fn emit_schema_fn_body_ext(
    fields: &[&&Field],
    all_optional: bool,
    extra_required: &[&&Field],
) -> TokenStream {
    let insertions: Vec<TokenStream> = fields
        .iter()
        .chain(extra_required.iter())
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let field_name = ident.to_string();
            let schema_expr = emit_json_schema_tokens(&f.ty);
            quote! {
                __props.insert(#field_name.to_owned(), #schema_expr);
            }
        })
        .collect();

    let mut required_names: Vec<String> = if all_optional {
        Vec::new()
    } else {
        fields
            .iter()
            .filter(|f| !is_option_type(&f.ty))
            .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
            .collect()
    };
    for f in extra_required {
        if let Some(id) = f.ident.as_ref() {
            required_names.push(id.to_string());
        }
    }

    let required_tokens: Vec<TokenStream> = required_names
        .iter()
        .map(|name| {
            quote! { ::autumn_web::reexports::serde_json::json!(#name) }
        })
        .collect();

    quote! {
        let mut __props = ::autumn_web::reexports::serde_json::Map::new();
        #(#insertions)*
        let mut __schema = ::autumn_web::reexports::serde_json::Map::new();
        __schema.insert(
            "type".to_owned(),
            ::autumn_web::reexports::serde_json::json!("object"),
        );
        __schema.insert(
            "properties".to_owned(),
            ::autumn_web::reexports::serde_json::Value::Object(__props),
        );
        let __required: ::std::vec::Vec<::autumn_web::reexports::serde_json::Value> =
            ::std::vec![#(#required_tokens),*];
        if !__required.is_empty() {
            __schema.insert(
                "required".to_owned(),
                ::autumn_web::reexports::serde_json::Value::Array(__required),
            );
        }
        ::autumn_web::reexports::serde_json::Value::Object(__schema)
    }
}

// ── State machine support ────────────────────────────────────────────────────

/// A single allowed transition between two named states.
struct StateMachineTransition {
    from: String,
    to: String,
    /// Optional guard: name of a `&self` bool method that must return `true`.
    guard: Option<String>,
}

/// Parsed `#[state_machine(transitions(...))]` spec for one field.
struct StateMachineSpec {
    field_ident: syn::Ident,
    transitions: Vec<StateMachineTransition>,
}

/// Parse the inner `transitions(a -> b, b -> c: "guard", ...)` token tree.
fn parse_transitions(
    input: syn::parse::ParseStream<'_>,
) -> syn::Result<Vec<StateMachineTransition>> {
    let kw: syn::Ident = input.parse()?;
    if kw != "transitions" {
        return Err(syn::Error::new(kw.span(), "expected `transitions(...)`"));
    }
    let content;
    syn::parenthesized!(content in input);

    let mut transitions = Vec::new();
    while !content.is_empty() {
        let from: syn::Ident = content.parse()?;
        content.parse::<syn::Token![->]>()?;
        let to: syn::Ident = content.parse()?;
        let guard = if content.peek(syn::Token![:]) {
            content.parse::<syn::Token![:]>()?;
            let lit: syn::LitStr = content.parse()?;
            Some(lit.value())
        } else {
            None
        };
        transitions.push(StateMachineTransition {
            from: from.to_string(),
            to: to.to_string(),
            guard,
        });
        if content.peek(syn::Token![,]) {
            content.parse::<syn::Token![,]>()?;
        }
    }
    Ok(transitions)
}

/// Parse `#[state_machine(transitions(...))]` from a field, returning the spec when present.
///
/// Validates:
/// - Only `String` fields are supported (the generated `.as_str()` call requires it).
/// - Multiple `#[state_machine]` attributes on the same field are rejected.
fn parse_state_machine_spec(field: &syn::Field) -> syn::Result<Option<StateMachineSpec>> {
    let Some(ident) = field.ident.as_ref() else {
        return Ok(None);
    };
    let mut spec: Option<StateMachineSpec> = None;
    for attr in &field.attrs {
        if attr.path().is_ident("state_machine") {
            if spec.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "multiple `#[state_machine]` attributes are not allowed on a single field",
                ));
            }
            let is_string = matches!(&field.ty, syn::Type::Path(p)
                if p.path.segments.last().is_some_and(|s| s.ident == "String"));
            if !is_string {
                return Err(syn::Error::new_spanned(
                    &field.ty,
                    "`#[state_machine]` is only supported on `String` fields",
                ));
            }
            let transitions = attr.parse_args_with(parse_transitions)?;
            spec = Some(StateMachineSpec {
                field_ident: ident.clone(),
                transitions,
            });
        }
    }
    Ok(spec)
}

/// Emit the three state machine items for one field: a transitions constant,
/// a `can_transition_{field}_to` predicate, and a `transition_{field}_to` method.
fn emit_state_machine_impl(model_name: &syn::Ident, spec: &StateMachineSpec) -> TokenStream {
    let field = &spec.field_ident;
    let field_str = field.to_string();
    let field_upper = field_str.to_uppercase();

    let const_name = format_ident!("__AUTUMN_SM_{field_upper}_TRANSITIONS");
    let can_fn = format_ident!("can_transition_{field_str}_to");
    let transition_fn = format_ident!("transition_{field_str}_to");

    let const_entries: Vec<TokenStream> = spec
        .transitions
        .iter()
        .map(|t| {
            let from = &t.from;
            let to = &t.to;
            t.guard.as_ref().map_or_else(
                || quote! { (#from, #to, ::core::option::Option::None) },
                |g| quote! { (#from, #to, ::core::option::Option::Some(#g)) },
            )
        })
        .collect();

    let match_arms: Vec<TokenStream> = spec
        .transitions
        .iter()
        .map(|t| {
            let from = &t.from;
            let to = &t.to;
            t.guard.as_ref().map_or_else(
                || quote! { (#from, #to) => true },
                |g| {
                    let guard_fn = format_ident!("{g}");
                    quote! { (#from, #to) => self.#guard_fn() }
                },
            )
        })
        .collect();

    quote! {
        impl #model_name {
            #[doc(hidden)]
            pub const #const_name: &'static [(&'static str, &'static str, ::core::option::Option<&'static str>)] = &[
                #(#const_entries,)*
            ];

            /// Returns `true` when this record's `{field}` can transition to `target`.
            ///
            /// For guarded transitions the corresponding guard method is called first.
            pub fn #can_fn(&self, target: &str) -> bool {
                match (self.#field.as_str(), target) {
                    #(#match_arms,)*
                    _ => false,
                }
            }

            /// Attempts to transition `{field}` to `target`, returning the new state value.
            ///
            /// Returns `Err` if the transition is not defined or a guard rejects it.
            pub fn #transition_fn(&self, target: &str) -> ::autumn_web::AutumnResult<::std::string::String> {
                if self.#can_fn(target) {
                    ::core::result::Result::Ok(::std::string::String::from(target))
                } else {
                    ::core::result::Result::Err(::autumn_web::AutumnError::bad_request_msg(
                        ::std::format!(
                            "Cannot transition `{}` from `{}` to `{}`",
                            #field_str,
                            self.#field,
                            target,
                        ),
                    ))
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::cognitive_complexity)]
pub fn model_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    let syn::Data::Struct(syn::DataStruct {
        fields: syn::Fields::Named(ref fields),
        ..
    }) = input.data
    else {
        return syn::Error::new_spanned(
            &input.ident,
            "#[model] can only be applied to structs with named fields",
        )
        .to_compile_error();
    };

    let table_name = match parse_attr_args(attr) {
        Ok(explicit) => explicit.unwrap_or_else(|| infer_table_name(&input.ident)),
        Err(err) => return err.to_compile_error(),
    };

    let table_ident = syn::Ident::new(&table_name, input.ident.span());
    let name = &input.ident;
    let vis = &input.vis;
    let outer_attrs = &input.attrs;

    let searchable_lang = match parse_model_searchable_lang(outer_attrs) {
        Ok(lang) => lang,
        Err(err) => return err.to_compile_error(),
    };
    let is_searchable = searchable_lang.is_some();
    let search_language = searchable_lang.unwrap_or_else(|| "simple".to_string());
    let filtered_outer_attrs: Vec<&syn::Attribute> = outer_attrs
        .iter()
        .filter(|a| !a.path().is_ident("searchable"))
        .collect();

    let new_name = format_ident!("New{name}");
    let update_name = format_ident!("Update{name}");
    let changeset_name = format_ident!("__{}Changeset", name);

    // Classify fields
    let all_fields: Vec<&Field> = fields.named.iter().collect();

    // Collect state machine specs from all fields (RED → GREEN: declarative SM support).
    let mut state_machine_impls: Vec<TokenStream> = Vec::new();
    for field in &all_fields {
        match parse_state_machine_spec(field) {
            Ok(Some(spec)) => {
                state_machine_impls.push(emit_state_machine_impl(name, &spec));
            }
            Ok(None) => {}
            Err(err) => return err.to_compile_error(),
        }
    }

    let mut search_field_names = Vec::new();
    let mut search_field_weights = Vec::new();

    for field in &all_fields {
        match parse_field_searchable_weight(field) {
            Ok(FieldSearchable::NotSearchable) => {}
            Ok(weight_type) => {
                let field_ident = field.ident.as_ref().unwrap();
                let weight = match weight_type {
                    FieldSearchable::SearchableWithWeight(w) => w,
                    FieldSearchable::SearchableDefault | FieldSearchable::NotSearchable => {
                        "D".to_string()
                    }
                };
                if weight.len() != 1 {
                    return syn::Error::new_spanned(
                        field_ident,
                        "searchable weight must be a single character (A, B, C, or D)",
                    )
                    .to_compile_error();
                }
                let weight_char = weight.chars().next().unwrap();
                if !['A', 'B', 'C', 'D'].contains(&weight_char) {
                    return syn::Error::new_spanned(
                        field_ident,
                        "searchable weight must be A, B, C, or D",
                    )
                    .to_compile_error();
                }
                search_field_names.push(field_ident.to_string());
                search_field_weights.push(weight_char);
            }
            Err(err) => return err.to_compile_error(),
        }
    }

    let id_fields: Vec<&&Field> = all_fields.iter().filter(|f| has_attr(f, "id")).collect();

    // If no explicit #[id], default to first i32/i64 field
    let id_field_names: Vec<&syn::Ident> = if id_fields.is_empty() {
        all_fields
            .iter()
            .filter(|f| {
                if let syn::Type::Path(tp) = &f.ty {
                    tp.path.is_ident("i32") || tp.path.is_ident("i64")
                } else {
                    false
                }
            })
            .take(1)
            .filter_map(|f| f.ident.as_ref())
            .collect()
    } else {
        id_fields.iter().filter_map(|f| f.ident.as_ref()).collect()
    };

    // PK field ident + type — used by the test-support factory to generate
    // `__autumn_pk()` so association setters don't hardcode `.id`.
    let pk_field_for_factory: Option<(&syn::Ident, &syn::Type)> = if id_fields.is_empty() {
        all_fields
            .iter()
            .find(|f| {
                if let syn::Type::Path(tp) = &f.ty {
                    tp.path.is_ident("i32") || tp.path.is_ident("i64")
                } else {
                    false
                }
            })
            .and_then(|f| f.ident.as_ref().map(|id| (id, &f.ty)))
    } else {
        id_fields
            .first()
            .and_then(|f| f.ident.as_ref().map(|id| (id, &f.ty)))
    };

    // Fields for NewX: exclude #[id], #[default], #[lock_version], and auto-detected ID fields
    let fields_for_new: Vec<&&Field> = all_fields
        .iter()
        .filter(|f| {
            !excluded_from_new(f)
                && f.ident
                    .as_ref()
                    .is_some_and(|id| !id_field_names.contains(&id))
        })
        .collect();

    // The single #[lock_version] field (if any). Only one is supported; the
    // first one wins. The field is excluded from NewX but is included in
    // UpdateX as a plain (non-Patch) required field so the client always
    // sends the version they read.
    let lock_version_field: Option<&&Field> =
        all_fields.iter().find(|f| has_attr(f, "lock_version"));

    // Validate #[factory_assoc] attributes before using them.
    if let Some(err) = validate_factory_assoc_attrs(&all_fields) {
        return err;
    }

    // Collect `#[encrypted]` columns (validated to be non-null `String`).
    // Each entry: (column, deterministic, admin_visible, versioned_ciphertext).
    let mut encrypted_columns: Vec<(String, bool, bool, bool)> = Vec::new();
    for f in &all_fields {
        if let Err(err) = validate_encrypted_field(f) {
            return err.to_compile_error();
        }
        match parse_field_encrypted(f) {
            Ok(spec) if spec.is_encrypted() => {
                let col = f.ident.as_ref().unwrap().to_string();
                encrypted_columns.push((
                    col,
                    spec.mode == EncryptedMode::Deterministic,
                    spec.admin_visible,
                    spec.versioned_ciphertext,
                ));
            }
            Ok(_) => {}
            Err(err) => return err.to_compile_error(),
        }
    }
    // A struct-level `#[serde(rename_all = ...)]` also desyncs encrypted-column
    // registration (Rust name) from the serialized key — reject it when any field
    // is encrypted (see `field_has_serde_rename` for the per-field case).
    if !encrypted_columns.is_empty() && attrs_have_serde_rename_all(outer_attrs) {
        return syn::Error::new_spanned(
            name,
            "`#[serde(rename_all = ...)]` cannot be combined with `#[encrypted]` fields in v1: \
             encrypted columns are registered under their Rust names, which must match the \
             serialized keys used by version history / log scrubbing / admin redaction.",
        )
        .to_compile_error();
    }
    let encrypted_column_names: Vec<&str> =
        encrypted_columns.iter().map(|(c, ..)| c.as_str()).collect();
    // Diesel's `AsChangeset`/`Insertable` derives expand `column.eq(value)` in
    // the model's module scope when `serialize_as` is present, which needs
    // `ExpressionMethods` in scope. Bring it in anonymously (only for models with
    // encrypted columns) so app authors don't have to add the import themselves.
    let encrypted_use = if encrypted_columns.is_empty() {
        quote! {}
    } else {
        quote! {
            #[allow(unused_imports)]
            use ::autumn_web::reexports::diesel::ExpressionMethods as _;
        }
    };
    // Encrypt encrypted columns in the durable commit-hook payload so secrets are
    // never persisted in plaintext to `autumn_repository_commit_hooks` (#805).
    let commit_hook_encrypt_stmt = if encrypted_columns.is_empty() {
        quote! {}
    } else {
        quote! {
            ::autumn_web::encryption::encrypt_persisted_columns_in_value(
                #table_name,
                &mut __autumn_value,
            );
        }
    };
    // Symmetric inverse: when a durable commit-hook record is read back to drive
    // `after_*_commit`, decrypt the encrypted columns first so replayed hooks
    // receive plaintext model values, exactly as on the normal repository path.
    let commit_hook_decrypt_stmt = if encrypted_columns.is_empty() {
        quote! {}
    } else {
        quote! {
            ::autumn_web::encryption::decrypt_persisted_columns_in_value(
                #table_name,
                &mut __autumn_decoded_value,
            );
        }
    };
    // For models with encrypted columns, replace the derived `Debug` on every
    // plaintext-holding struct (query, New*, Update*, Changeset) with a redacting
    // manual impl so values never leak through `Debug`/panic output — including
    // update payloads whose `Patch<String>` would otherwise print `Set("secret")`
    // (#805 AC, composes with #697).
    let lock_version_ident: Option<&syn::Ident> = lock_version_field.and_then(|f| f.ident.as_ref());
    let mutable_idents: Vec<&syn::Ident> = fields_for_new
        .iter()
        .map(|f| f.ident.as_ref().unwrap())
        .chain(lock_version_ident)
        .collect();
    let (
        name_debug_derive,
        name_debug_impl,
        new_debug_derive,
        new_debug_impl,
        update_debug_derive,
        update_debug_impl,
        changeset_debug_derive,
        changeset_debug_impl,
    ) = if encrypted_columns.is_empty() {
        (
            quote! { Debug, },
            quote! {},
            quote! { Debug, },
            quote! {},
            quote! { Debug, },
            quote! {},
            quote! { Debug, },
            quote! {},
        )
    } else {
        let all_idents: Vec<&syn::Ident> = all_fields
            .iter()
            .map(|f| f.ident.as_ref().unwrap())
            .collect();
        let new_idents: Vec<&syn::Ident> = fields_for_new
            .iter()
            .map(|f| f.ident.as_ref().unwrap())
            .collect();
        (
            quote! {},
            redacting_debug_impl(name, &all_idents, &encrypted_column_names),
            quote! {},
            redacting_debug_impl(&new_name, &new_idents, &encrypted_column_names),
            quote! {},
            redacting_debug_impl(&update_name, &mutable_idents, &encrypted_column_names),
            quote! {},
            redacting_debug_impl(&changeset_name, &mutable_idents, &encrypted_column_names),
        )
    };
    let encrypted_inventory: Vec<TokenStream> = encrypted_columns
        .iter()
        .map(
            |(col, deterministic, admin_visible, versioned_ciphertext)| {
                quote! {
                    ::autumn_web::reexports::inventory::submit! {
                        ::autumn_web::encryption::EncryptedColumnDescriptor {
                            model: stringify!(#name),
                            table: #table_name,
                            column: #col,
                            deterministic: #deterministic,
                            admin_visible: #admin_visible,
                            versioned_ciphertext: #versioned_ciphertext,
                        }
                    }
                }
            },
        )
        .collect();

    // Fields for UpdateX: Patch fields (from fields_for_new) plus the
    // lock_version field (plain required type, not Patch<T>).

    // Check if any field has #[validate(...)]
    let has_validation = all_fields.iter().any(|f| !validate_attrs(f).is_empty());

    // Build query struct fields (strip #[id], #[indexed], #[validate])
    let query_fields: Vec<TokenStream> = all_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            let attrs = user_attrs(f);
            // Encrypted columns route through an AEAD wrapper transparently:
            // `serialize_as` encrypts on write, `deserialize_as` decrypts on read.
            // The public field stays a plain `String` (plaintext in Rust code).
            let enc = encrypted_wrapper_path(
                parse_field_encrypted_mode(f).unwrap_or(EncryptedMode::None),
            )
            .map(|w| quote! { #[diesel(serialize_as = #w, deserialize_as = #w)] });
            quote! { #(#attrs)* #enc pub #ident: #ty }
        })
        .collect();

    // Build NewX fields (non-ID, propagate #[validate])
    let new_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            let val_attrs = validate_attrs(f);
            let enc = encrypted_wrapper_path(
                parse_field_encrypted_mode(f).unwrap_or(EncryptedMode::None),
            )
            .map(|w| quote! { #[diesel(serialize_as = #w)] });
            quote! { #(#val_attrs)* #enc pub #ident: #ty }
        })
        .collect();

    // Build UpdateX fields:
    // - Regular mutable fields: Patch<T> (no #[validate] — validation only
    //   applies to NewX and the merged model)
    // - #[lock_version] field: plain required T (the client supplies the
    //   version they read; the framework increments it atomically)
    let mut update_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            quote! {
                #[serde(default)]
                pub #ident: ::autumn_web::hooks::Patch<#ty>
            }
        })
        .collect();
    if let Some(lv_field) = lock_version_field {
        let ident = &lv_field.ident;
        let ty = &lv_field.ty;
        update_fields.push(quote! {
            pub #ident: #ty
        });
    }

    // Build XField enum variants (one per mutable field, PascalCase)
    let field_enum_name = format_ident!("{name}Field");
    let field_enum_variants: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let variant = format_ident!("{}", pascal_case(&ident.to_string()));
            quote! { #variant }
        })
        .collect();

    // Conditional Validate derive
    let validate_derive = if has_validation {
        quote! { #[derive(::autumn_web::reexports::validator::Validate)] }
    } else {
        quote! {}
    };

    // Build merge arms for `from_patch` (applies Patch fields onto a cloned model)
    let mut merge_arms: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let is_option = is_option_type(&f.ty);
            if is_option {
                quote! {
                    match &patch.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => after.#ident = v.clone(),
                        ::autumn_web::hooks::Patch::Clear => after.#ident = None,
                        ::autumn_web::hooks::Patch::Unchanged => {}
                    }
                }
            } else {
                quote! {
                    match &patch.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => after.#ident = v.clone(),
                        ::autumn_web::hooks::Patch::Clear => {
                            return Err(::autumn_web::AutumnError::bad_request_msg(
                                format!("Cannot clear non-nullable field `{}`", stringify!(#ident))
                            ));
                        }
                        ::autumn_web::hooks::Patch::Unchanged => {}
                    }
                }
            }
        })
        .collect();
    // For #[lock_version] fields, from_patch always increments the version in
    // `after` by one — the client-supplied patch.{field} is the expected
    // (before) version; the repository validates it and the changeset carries
    // the incremented value into the DB.
    if let Some(lv_field) = lock_version_field {
        let ident = lv_field.ident.as_ref().unwrap();
        merge_arms.push(quote! {
            after.#ident = current.#ident.wrapping_add(1);
        });
    }

    // Build per-field DraftField accessor method signatures (for the trait)
    let draft_accessor_sigs: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;
            quote! {
                fn #ident(&mut self) -> ::autumn_web::hooks::DraftField<'_, #ty>;
            }
        })
        .collect();

    // Build per-field DraftField accessor method implementations
    let draft_accessors: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;
            quote! {
                fn #ident(&mut self) -> ::autumn_web::hooks::DraftField<'_, #ty> {
                    ::autumn_web::hooks::DraftField::new(&self.before.#ident, &mut self.after.#ident)
                }
            }
        })
        .collect();

    // Trait name for draft extension methods
    let draft_ext_name = format_ident!("{name}DraftExt");

    let column_count = all_fields.len();
    let new_column_count = fields_for_new.len();

    // Build Diesel-compatible changeset bridge (private struct with Option<T> fields)
    // (`changeset_name` is bound earlier so the redacting Debug impl can use it.)

    let tenant_id_field = all_fields
        .iter()
        .find(|f| f.ident.as_ref().is_some_and(|id| id == "tenant_id"))
        .copied();

    let new_has_tenant_id = fields_for_new
        .iter()
        .any(|f| f.ident.as_ref().is_some_and(|id| id == "tenant_id"));

    let can_set_tenant_id_impl = if new_has_tenant_id {
        let f = fields_for_new
            .iter()
            .find(|f| f.ident.as_ref().is_some_and(|id| id == "tenant_id"))
            .unwrap();
        let is_option = is_option_type(&f.ty);
        let val = if is_option {
            quote! { ::core::option::Option::Some(::core::option::Option::Some(t)) }
        } else {
            quote! { ::core::option::Option::Some(t) }
        };
        quote! {
            impl ::autumn_web::repository::CanSetTenantId for #changeset_name {
                fn set_tenant_id(&mut self, t: ::std::string::String) {
                    self.tenant_id = #val;
                }
            }
        }
    } else {
        quote! {
            impl ::autumn_web::repository::CanSetTenantId for #changeset_name {
                fn set_tenant_id(&mut self, _t: ::std::string::String) {}
            }
        }
    };

    let model_tenant_id_meta_impl = tenant_id_field.as_ref().map_or_else(
        || {
            quote! {
                impl ::autumn_web::tenancy::ModelTenantIdMeta for #new_name {
                    const HAS_MANUAL_TENANT_ID: bool = false;
                    fn try_set_tenant_id(&mut self, _tenant_id: &str) {}
                }
                impl ::autumn_web::tenancy::ModelTenantIdMeta for #name {
                    const HAS_MANUAL_TENANT_ID: bool = false;
                    fn try_set_tenant_id(&mut self, _tenant_id: &str) {}
                }
            }
        },
        |f| {
            let is_option = is_option_type(&f.ty);
            let set_field = if is_option {
                quote! { self.tenant_id = ::core::option::Option::Some(tenant_id.to_string()); }
            } else {
                quote! { self.tenant_id = tenant_id.to_string(); }
            };

            let new_set_field = if new_has_tenant_id {
                set_field.clone()
            } else {
                quote! {}
            };

            quote! {
                impl ::autumn_web::tenancy::ModelTenantIdMeta for #new_name {
                    const HAS_MANUAL_TENANT_ID: bool = #new_has_tenant_id;
                    fn try_set_tenant_id(&mut self, tenant_id: &str) {
                        #new_set_field
                    }
                }
                impl ::autumn_web::tenancy::ModelTenantIdMeta for #name {
                    const HAS_MANUAL_TENANT_ID: bool = true;
                    fn try_set_tenant_id(&mut self, tenant_id: &str) {
                        #set_field
                    }
                }
            }
        },
    );

    let mut upsert_columns: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            quote! {
                #table_ident::#ident.eq(::autumn_web::reexports::diesel::upsert::excluded(#table_ident::#ident))
            }
        })
        .collect();

    if upsert_columns.is_empty() {
        upsert_columns.push(quote! {
            #table_ident::id.eq(::autumn_web::reexports::diesel::pg::upsert::excluded(#table_ident::id))
        });
    }

    if let Some(lv_field) = lock_version_field {
        let ident = lv_field.ident.as_ref().unwrap();
        upsert_columns.push(quote! {
            #table_ident::#ident.eq(#table_ident::#ident + 1)
        });
    }

    let mut upsert_types: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            quote! {
                ::autumn_web::reexports::diesel::dsl::Eq<
                    #table_ident::#ident,
                    ::autumn_web::reexports::diesel::upsert::Excluded<#table_ident::#ident>
                >
            }
        })
        .collect();

    if upsert_types.is_empty() {
        upsert_types.push(quote! {
            ::autumn_web::reexports::diesel::dsl::Eq<
                #table_ident::id,
                ::autumn_web::reexports::diesel::upsert::Excluded<#table_ident::id>
            >
        });
    }

    if let Some(lv_field) = lock_version_field {
        let ident = lv_field.ident.as_ref().unwrap();
        let ty = &lv_field.ty;
        upsert_types.push(quote! {
            ::autumn_web::reexports::diesel::dsl::Eq<
                #table_ident::#ident,
                ::autumn_web::reexports::diesel::helper_types::Add<
                    #table_ident::#ident,
                    ::autumn_web::reexports::diesel::expression::bound::Bound<
                        <#table_ident::#ident as ::autumn_web::reexports::diesel::Expression>::SqlType,
                        #ty
                    >
                >
            >
        });
    }

    let has_tenant_id = tenant_id_field.is_some();
    let execute_upsert_body = if has_tenant_id {
        lock_version_field.map_or_else(
            || quote! {
                if let ::core::option::Option::Some(t) = tenant_id {
                    let stmt = ::autumn_web::reexports::diesel::query_dsl::methods::FilterDsl::filter(stmt, #table_ident::tenant_id.eq(t.to_string()));
                    stmt.get_results::<Self>(conn).await
                } else {
                    stmt.get_results::<Self>(conn).await
                }
            },
            |lv_field| {
                let lv_ident = lv_field.ident.as_ref().unwrap();
                quote! {
                    let lv_cond = #table_ident::#lv_ident.eq(::autumn_web::reexports::diesel::pg::upsert::excluded(#table_ident::#lv_ident));
                    if let ::core::option::Option::Some(t) = tenant_id {
                        let stmt = ::autumn_web::reexports::diesel::query_dsl::methods::FilterDsl::filter(stmt, lv_cond.and(#table_ident::tenant_id.eq(t.to_string())));
                        stmt.get_results::<Self>(conn).await
                    } else {
                        let stmt = ::autumn_web::reexports::diesel::query_dsl::methods::FilterDsl::filter(stmt, lv_cond);
                        stmt.get_results::<Self>(conn).await
                    }
                }
            },
        )
    } else {
        lock_version_field.map_or_else(
            || quote! {
                stmt.get_results::<Self>(conn).await
            },
            |lv_field| {
                let lv_ident = lv_field.ident.as_ref().unwrap();
                quote! {
                    let lv_cond = #table_ident::#lv_ident.eq(::autumn_web::reexports::diesel::pg::upsert::excluded(#table_ident::#lv_ident));
                    let stmt = ::autumn_web::reexports::diesel::query_dsl::methods::FilterDsl::filter(stmt, lv_cond);
                    stmt.get_results::<Self>(conn).await
                }
            },
        )
    };

    let compare_fields = fields_for_new.iter().map(|f| {
        let ident = &f.ident;
        quote! { input.#ident == record.#ident }
    });
    let compare_expr = if fields_for_new.is_empty() {
        quote! { true }
    } else {
        quote! { #(#compare_fields)&&* }
    };

    let mut changeset_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            // For both nullable and non-nullable columns, Diesel's AsChangeset
            // treats Option<T> as "skip if None, set if Some". For nullable
            // columns (Option<Inner>), this becomes Option<Option<Inner>> which
            // also handles "set to NULL" via Some(None).
            //
            // For encrypted columns the inner value is routed through the AEAD
            // wrapper via `serialize_as` (Diesel maps the `Option` skip itself),
            // so updates write ciphertext while the API stays plaintext.
            let enc = encrypted_wrapper_path(
                parse_field_encrypted_mode(f).unwrap_or(EncryptedMode::None),
            )
            .map(|w| quote! { #[diesel(serialize_as = #w)] });
            quote! { #enc pub #ident: Option<#ty> }
        })
        .collect();
    // The lock_version column must be in the changeset so the UPDATE can
    // atomically bump it to current+1.
    if let Some(lv_field) = lock_version_field {
        let ident = &lv_field.ident;
        let ty = &lv_field.ty;
        changeset_fields.push(quote! { pub #ident: Option<#ty> });
    }

    let mut changeset_conversions: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let is_option = is_option_type(&f.ty);
            if is_option {
                // For nullable fields: Set(v) -> Some(v), Clear -> Some(None), Unchanged -> None
                quote! {
                    #ident: match &self.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => Some(v.clone()),
                        ::autumn_web::hooks::Patch::Clear => Some(None),
                        ::autumn_web::hooks::Patch::Unchanged => None,
                    }
                }
            } else {
                // For non-nullable fields: Set(v) -> Some(v), Unchanged -> None, Clear -> panic
                quote! {
                    #ident: match &self.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => Some(v.clone()),
                        ::autumn_web::hooks::Patch::Clear => {
                            panic!("Cannot clear non-nullable field `{}`", stringify!(#ident));
                        }
                        ::autumn_web::hooks::Patch::Unchanged => None,
                    }
                }
            }
        })
        .collect();
    // The lock_version field in UpdateX holds the version the client expects;
    // the changeset always sets it to current+1 (wrapping to avoid overflow).
    if let Some(lv_field) = lock_version_field {
        let ident = lv_field.ident.as_ref().unwrap();
        changeset_conversions.push(quote! {
            #ident: Some(self.#ident.wrapping_add(1))
        });
    }

    // ── Factory builder ────────────────────────────────────────
    let factory_name = format_ident!("{name}Factory");

    // PK ident/type used for __autumn_pk() — fall back to a dummy `id: i64` if
    // no PK can be detected (the factory will fail to compile at the call site,
    // which is a better diagnostic than a macro panic).
    let (pk_id, pk_ty): (&syn::Ident, &syn::Type) = pk_field_for_factory.unwrap_or_else(|| {
        // Dummy values — unreachable for well-formed models, which always
        // have at least one i32/i64 field or an explicit #[id] annotation.
        panic!("#[model]: could not detect primary-key field for factory generation")
    });

    // Whether any factory field is an association (drives depth-check generation).
    let has_assoc_fields = fields_for_new
        .iter()
        .any(|f| factory_assoc_type(f).is_some());

    // Factory struct fields.
    // - Normal fields:  `pub {ident}: {ty}`
    // - Assoc fields:   `pub {ident}: Option<{ty}>` (None = auto-create on create())
    let factory_struct_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            if factory_assoc_type(f).is_some() {
                quote! { pub #ident: ::core::option::Option<#ty> }
            } else {
                quote! { pub #ident: #ty }
            }
        })
        .collect();

    // Default impl.
    // - Normal fields:  `{ident}: Default::default()`
    // - Assoc fields:   `{ident}: None`
    let factory_default_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            if factory_assoc_type(f).is_some() {
                quote! { #ident: ::core::option::Option::None }
            } else {
                quote! { #ident: ::core::default::Default::default() }
            }
        })
        .collect();

    // Per-field setter methods.
    // - Normal fields:  `pub fn {ident}(mut self, val: impl Into<T>) -> Self`
    // - Assoc fields:   same setter (stores `Some(val.into())`), PLUS
    //                   `pub fn {assoc_snake}(mut self, val: &AssocType) -> Self`
    //                   that extracts `.id` from a pre-built instance.
    let factory_setters: Vec<TokenStream> = fields_for_new
        .iter()
        .flat_map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;

            factory_assoc_type(f).map_or_else(
                // Normal field: a single setter that assigns directly.
                || {
                    vec![quote! {
                        #[must_use]
                        pub fn #ident(mut self, val: impl ::core::convert::Into<#ty>) -> Self {
                            self.#ident = val.into();
                            self
                        }
                    }]
                },
                // Assoc field: two setters — explicit id and pre-built instance.
                |assoc_type| {
                    let explicit_setter = quote! {
                        #[must_use]
                        pub fn #ident(mut self, val: impl ::core::convert::Into<#ty>) -> Self {
                            self.#ident = ::core::option::Option::Some(val.into());
                            self
                        }
                    };
                    // Name derived from the field ident by stripping the `_id` suffix:
                    // `user_id` → `.user()`, `author_id` → `.author()`.
                    let field_str = ident.to_string();
                    let assoc_snake = if field_str.ends_with("_id") {
                        format_ident!("{}", &field_str[..field_str.len() - 3])
                    } else {
                        format_ident!("{}_assoc", field_str)
                    };
                    let pre_built_setter = quote! {
                        /// Override the association with a pre-built instance.
                        /// Extracts the primary key so no additional DB insert is performed on `create()`.
                        #[must_use]
                        pub fn #assoc_snake(mut self, val: &#assoc_type) -> Self {
                            self.#ident = ::core::option::Option::Some(val.__autumn_pk());
                            self
                        }
                    };
                    vec![explicit_setter, pre_built_setter]
                },
            )
        })
        .collect();

    // build() — assemble NewX.
    // - Normal fields:  `{ident}: self.{ident}`
    // - Assoc fields:   `{ident}: self.{ident}.unwrap_or_default()`
    let factory_build_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            if factory_assoc_type(f).is_some() {
                quote! { #ident: self.#ident.unwrap_or_default() }
            } else {
                quote! { #ident: self.#ident }
            }
        })
        .collect();

    // create() — auto-resolve assoc fields, then insert.
    //
    // For each assoc field, emit a `let __resolved_{ident}` that either uses the
    // supplied value or auto-creates the associated model via its factory.
    //
    // A thread-local depth counter guards against cyclic associations: if the
    // chain exceeds 32 levels the factory panics with a clear message rather than
    // overflowing the stack.
    let assoc_resolutions: Vec<TokenStream> = fields_for_new
        .iter()
        .filter_map(|f| {
            let assoc_type = factory_assoc_type(f)?;
            let ident = f.ident.as_ref().unwrap();
            let resolved = format_ident!("__resolved_{ident}");
            Some(quote! {
                let #resolved = match self.#ident {
                    ::core::option::Option::Some(id) => id,
                    ::core::option::Option::None => {
                        #assoc_type::factory().create(pool).await.__autumn_pk()
                    }
                };
            })
        })
        .collect();

    // NewX construction inside create() uses resolved values for assoc fields.
    let create_build_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            if factory_assoc_type(f).is_some() {
                let resolved = format_ident!("__resolved_{ident}");
                quote! { #ident: #resolved }
            } else {
                quote! { #ident: self.#ident }
            }
        })
        .collect();

    // create() inner body — shared by both the assoc and non-assoc paths.
    let create_inner_body = quote! {
        use ::autumn_web::reexports::diesel::prelude::*;
        use ::autumn_web::reexports::diesel_async::RunQueryDsl;

        #(#assoc_resolutions)*

        let new_record = #new_name {
            #(#create_build_fields,)*
        };
        let mut conn = pool
            .get()
            .await
            .expect("factory: failed to acquire db connection");
        ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
            // Owned (not `&new_record`): encrypted columns route through diesel
            // `serialize_as`, which consumes the value, so `Insertable` is only
            // implemented for the owned record. Owned also works for plain models.
            .values(new_record)
            .returning(#name::as_returning())
            .get_result(&mut *conn)
            .await
            .expect("factory: insert failed")
    };

    // create() — insert via Diesel and return the persisted model.
    //
    // For models with #[factory_assoc] fields, the body is wrapped in a
    // `tokio::task_local` scope so the depth counter is maintained correctly
    // when the future migrates between worker threads (work-stealing runtimes).
    // Thread-local storage would corrupt the counter across await points.
    let factory_create_method = if has_assoc_fields {
        quote! {
            /// Insert a record built from this factory into the database and return
            /// the fully-populated model (with server-assigned primary key).
            ///
            /// Fields annotated with `#[factory_assoc(Type)]` are auto-created via
            /// `Type::factory().create(pool).await` when no explicit value was set.
            /// Supply a pre-built instance with the `.{type_snake}(instance)` setter
            /// to skip the extra insert.
            ///
            /// Panics if the insert fails or if a cyclic association chain is detected
            /// (depth > 32).
            pub async fn create(
                self,
                pool: &::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                    ::autumn_web::reexports::diesel_async::AsyncPgConnection,
                >,
            ) -> #name {
                let __depth = ::autumn_web::__private::FACTORY_DEPTH
                    .try_with(|d| d + 1)
                    .unwrap_or(1_u32);
                assert!(
                    __depth <= 32,
                    "factory `{}`: cyclic #[factory_assoc] chain exceeds depth 32 — \
                     break the cycle by supplying a pre-built instance via a pre-built setter.",
                    stringify!(#name),
                );
                ::autumn_web::__private::FACTORY_DEPTH
                    .scope(__depth, async move { #create_inner_body })
                    .await
            }
        }
    } else {
        quote! {
            /// Insert a record built from this factory into the database and return
            /// the fully-populated model (with server-assigned primary key).
            ///
            /// Panics if the insert fails.
            pub async fn create(
                self,
                pool: &::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                    ::autumn_web::reexports::diesel_async::AsyncPgConnection,
                >,
            ) -> #name {
                #create_inner_body
            }
        }
    };

    // ── Optimistic-lock helper bodies ──────────────────────────────────────
    // Generate the bodies for the two hidden lock-version methods.
    // For models without #[lock_version] both bodies return `None` so the
    // repository macro can call them unconditionally.
    let lock_version_actual_body: TokenStream = lock_version_field.map_or_else(
        || quote! { ::core::option::Option::None },
        |lv_field| {
            let ident = lv_field.ident.as_ref().unwrap();
            quote! { ::core::option::Option::Some(self.#ident as i64) }
        },
    );

    let lock_version_expected_body: TokenStream = lock_version_field.map_or_else(
        || quote! { ::core::option::Option::None },
        |lv_field| {
            let ident = lv_field.ident.as_ref().unwrap();
            quote! { ::core::option::Option::Some(self.#ident as i64) }
        },
    );

    // Generate `pub fn etag(&self) -> ::autumn_web::etag::ETag` only when the
    // model carries a `#[lock_version]` field.  For models without one, the
    // method is omitted entirely — it would be meaningless.
    let etag_method: TokenStream = lock_version_field.map_or_else(
        || quote! {},
        |lv_field| {
            let ident = lv_field.ident.as_ref().unwrap();
            quote! {
                /// Derive an ETag from this model's lock version.
                ///
                /// Use with `autumn_web::etag::fresh_when` for one-liner
                /// conditional-GET support:
                ///
                /// ```rust,ignore
                /// let fw = fresh_when(&headers, post.etag());
                /// Ok(fw.or(html! { ... }))
                /// ```
                ///
                /// The ETag is deterministic: same `lock_version` ⇒ same ETag
                /// on every replica, with no dependence on wall clock or RNG.
                #[inline]
                pub fn etag(&self) -> ::autumn_web::etag::ETag {
                    ::autumn_web::etag::IntoETag::into_etag(self.#ident as i64)
                }
            }
        },
    );

    // Compute schema bodies for OpenApiSchema impls.
    // all_fields is Vec<&Field>; emit_schema_fn_body expects &[&&Field].
    let all_field_refs: Vec<&&Field> = all_fields.iter().collect();
    let query_struct_schema_body = emit_schema_fn_body(&all_field_refs, false);
    let new_struct_schema_body = emit_schema_fn_body(&fields_for_new, false);
    let update_struct_schema_body = {
        let extra: &[&&Field] = lock_version_field.as_slice();
        emit_schema_fn_body_ext(&fields_for_new, true, extra)
    };
    let commit_hook_serialize_fields: Vec<TokenStream> = all_fields
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().expect("named field");
            let ty = &f.ty;
            let field_name = LitStr::new(&ident.to_string(), ident.span());
            let field_value = if has_hook_serde_adapter(f, SerdeAdapterMode::Serialize) {
                let serde_attrs = hook_serde_adapter_attrs(f, SerdeAdapterMode::Serialize);
                quote! {
                    {
                        #[derive(::serde::Serialize)]
                        struct __AutumnCommitHookSerializeField {
                            #(#serde_attrs)*
                            value: #ty,
                        }
                        let __autumn_field = __AutumnCommitHookSerializeField {
                            value: self.#ident.clone(),
                        };
                        let __autumn_field_value =
                            ::autumn_web::reexports::serde_json::to_value(&__autumn_field)
                                .map_err(|__error| {
                                    ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                        "serialize repository commit hook record field {}.{}: {}",
                                        stringify!(#name),
                                        #field_name,
                                        __error
                                    ))
                                })?;
                        match __autumn_field_value {
                            ::autumn_web::reexports::serde_json::Value::Object(mut __autumn_field_object) => {
                                __autumn_field_object.remove("value").ok_or_else(|| {
                                    ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                        "serialize repository commit hook record field {}.{}: missing adapter output",
                                        stringify!(#name),
                                        #field_name
                                    ))
                                })?
                            }
                            __autumn_other => {
                                return Err(::autumn_web::AutumnError::internal_server_error_msg(format!(
                                    "serialize repository commit hook record field {}.{}: expected adapter object, got {}",
                                    stringify!(#name),
                                    #field_name,
                                    __autumn_other
                                )));
                            }
                        }
                    }
                }
            } else {
                quote! {
                    ::autumn_web::reexports::serde_json::to_value(&self.#ident)
                        .map_err(|__error| {
                            ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                "serialize repository commit hook record field {}.{}: {}",
                                stringify!(#name),
                                #field_name,
                                __error
                            ))
                        })?
                }
            };
            quote! {
                __autumn_object.insert(
                    ::std::string::String::from(#field_name),
                    #field_value
                );
            }
        })
        .collect();
    let commit_hook_deserialize_fields: Vec<TokenStream> = all_fields
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().expect("named field");
            let ty = &f.ty;
            let field_name = LitStr::new(&ident.to_string(), ident.span());
            let missing_default = commit_hook_missing_field_default_expr(f);
            let field_value = if has_hook_serde_adapter(f, SerdeAdapterMode::Deserialize) {
                let serde_attrs = hook_serde_adapter_attrs(f, SerdeAdapterMode::Deserialize);
                quote! {
                    {
                        #[derive(::serde::Deserialize)]
                        struct __AutumnCommitHookDeserializeField {
                            #(#serde_attrs)*
                            value: #ty,
                        }
                        let mut __autumn_wrapper_object =
                            ::autumn_web::reexports::serde_json::Map::new();
                        __autumn_wrapper_object.insert(
                            ::std::string::String::from("value"),
                            __autumn_field,
                        );
                        let __autumn_wrapper: __AutumnCommitHookDeserializeField =
                            ::autumn_web::reexports::serde_json::from_value(
                                ::autumn_web::reexports::serde_json::Value::Object(
                                    __autumn_wrapper_object,
                                ),
                            )
                            .map_err(|__error| {
                                ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                    "deserialize repository commit hook record field {}.{}: {}",
                                    stringify!(#name),
                                    #field_name,
                                    __error
                                ))
                            })?;
                        __autumn_wrapper.value
                    }
                }
            } else {
                quote! {
                    ::autumn_web::reexports::serde_json::from_value(__autumn_field)
                        .map_err(|__error| {
                            ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                "deserialize repository commit hook record field {}.{}: {}",
                                stringify!(#name),
                                #field_name,
                                __error
                            ))
                        })?
                }
            };
            missing_default.map_or_else(
                || {
                    quote! {
                    let #ident: #ty = {
                        let __autumn_field = __autumn_object.remove(#field_name)
                            .ok_or_else(|| {
                                ::autumn_web::AutumnError::internal_server_error_msg(format!(
                                    "deserialize repository commit hook record field {}.{}: missing field",
                                    stringify!(#name),
                                    #field_name
                                ))
                            })?;
                        #field_value
                    };
                }
                },
                |missing_default| {
                    quote! {
                    let #ident: #ty = match __autumn_object.remove(#field_name) {
                        ::core::option::Option::Some(__autumn_field) => {
                            #field_value
                        }
                        ::core::option::Option::None => {
                            #missing_default
                        }
                    };
                }
                },
            )
        })
        .collect();
    let commit_hook_construct_fields: Vec<TokenStream> = all_fields
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().expect("named field");
            quote! { #ident: #ident }
        })
        .collect();
    let commit_hook_serialize_bounds: Vec<TokenStream> = all_fields
        .iter()
        .filter(|f| !has_hook_serde_adapter(f, SerdeAdapterMode::Serialize))
        .map(|f| {
            let ty = &f.ty;
            quote! { #ty: ::serde::Serialize }
        })
        .collect();
    let mut commit_hook_deserialize_bounds: Vec<TokenStream> = all_fields
        .iter()
        .filter(|f| !has_hook_serde_adapter(f, SerdeAdapterMode::Deserialize))
        .map(|f| {
            let ty = &f.ty;
            quote! { #ty: ::serde::de::DeserializeOwned }
        })
        .collect();
    commit_hook_deserialize_bounds.extend(
        all_fields
            .iter()
            .filter(|f| !is_option_type(&f.ty))
            .filter(|f| matches!(serde_default_kind(f), Some(SerdeDefaultKind::Default)))
            .map(|f| {
                let ty = &f.ty;
                quote! { #ty: ::core::default::Default }
            }),
    );
    let commit_hook_serialize_where = if commit_hook_serialize_bounds.is_empty() {
        quote! {}
    } else {
        quote! { where #(#commit_hook_serialize_bounds,)* }
    };
    let commit_hook_deserialize_where = if commit_hook_deserialize_bounds.is_empty() {
        quote! {}
    } else {
        quote! { where #(#commit_hook_deserialize_bounds,)* }
    };

    quote! {
        #encrypted_use

        #[derive(#name_debug_derive Clone, ::diesel::Queryable, ::diesel::Selectable, ::diesel::AsChangeset, ::diesel::Insertable)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #[diesel(table_name = #table_ident)]
        #(#filtered_outer_attrs)*
        #vis struct #name {
            #(#query_fields,)*
        }
        #name_debug_impl

        #[derive(#new_debug_derive Clone, ::diesel::Insertable)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #validate_derive
        #[diesel(table_name = #table_ident)]
        #vis struct #new_name {
            #(#new_fields,)*
        }
        #new_debug_impl

        #[derive(#update_debug_derive Clone, Default)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #vis struct #update_name {
            #(#update_fields,)*
        }
        #update_debug_impl

        /// Diesel-compatible changeset derived from `Patch<T>` fields.
        ///
        /// This type bridges the `Patch`-based `UpdateX` and Diesel's
        /// `AsChangeset` trait. Use `UpdateX::__to_changeset()` to convert.
        #[doc(hidden)]
        #[derive(#changeset_debug_derive Clone, ::diesel::AsChangeset)]
        #[diesel(table_name = #table_ident)]
        pub struct #changeset_name {
            #(#changeset_fields,)*
        }
        #changeset_debug_impl

        impl #name {
            /// Column names on this model that are at-rest encrypted.
            ///
            /// Emitted for every model (empty when none are encrypted) so that
            /// version history, log scrubbing, and the admin plugin can redact
            /// encrypted columns by default. See `autumn_web::encryption`.
            #[doc(hidden)]
            pub const __AUTUMN_ENCRYPTED_COLUMNS: &'static [&'static str] =
                &[#(#encrypted_column_names),*];
        }

        #(#encrypted_inventory)*

        impl #update_name {
            #[doc(hidden)]
            #[must_use]
            pub fn __to_changeset(&self) -> #changeset_name {
                #changeset_name {
                    #(#changeset_conversions,)*
                }
            }
        }

        impl #name {
            pub const __AUTUMN_COLUMN_COUNT: usize = #column_count;

            #[doc(hidden)]
            pub fn __autumn_column_count(&self) -> usize {
                Self::__AUTUMN_COLUMN_COUNT
            }

            #[doc(hidden)]
            pub fn __autumn_upsert_set() -> impl ::autumn_web::reexports::diesel::query_builder::AsChangeset<
                Target = #table_ident::table,
                Changeset = impl ::autumn_web::reexports::diesel::query_builder::QueryFragment<::autumn_web::reexports::diesel::pg::Pg> + ::core::marker::Send + ::core::marker::Sync + 'static
            > + ::core::marker::Send + ::core::marker::Sync + 'static {
                use ::autumn_web::reexports::diesel::ExpressionMethods as _;
                (#(#upsert_columns,)*)
            }

            #[doc(hidden)]
            pub async fn __autumn_execute_upsert(
                chunk: &[Self],
                tenant_id: ::core::option::Option<&str>,
                conn: &mut ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            ) -> ::core::result::Result<::std::vec::Vec<Self>, ::autumn_web::reexports::diesel::result::Error> {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;

                let stmt = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                    // Owned `Vec` (not `&[Self]`): encrypted columns use diesel
                    // `serialize_as`, which only implements `Insertable` for owned
                    // values. `to_vec()` also works for plain models.
                    .values(chunk.to_vec())
                    .on_conflict(#table_ident::id)
                    .do_update()
                    .set(Self::__autumn_upsert_set());

                #execute_upsert_body
            }



            #[doc(hidden)]
            pub fn __autumn_correlate_new(
                inputs: &[#new_name],
                record: &Self,
                matched: &mut [bool],
            ) -> ::core::option::Option<usize> {
                for (i, input) in inputs.iter().enumerate() {
                    if !matched[i] {
                        if #compare_expr {
                            return ::core::option::Option::Some(i);
                        }
                    }
                }
                ::core::option::Option::None
            }

            #[doc(hidden)]
            pub fn __autumn_correlate_model(
                inputs: &[Self],
                record: &Self,
                matched: &mut [bool],
                ) -> ::core::option::Option<usize> {
                for (i, input) in inputs.iter().enumerate() {
                    if !matched[i] {
                        if #compare_expr {
                            return ::core::option::Option::Some(i);
                        }
                    }
                }
                ::core::option::Option::None
            }
        }

        impl ::autumn_web::repository::AutumnUpsertSetExt for #name {
            type UpsertSet = ::autumn_web::reexports::diesel::dsl::Eq<
                #table_ident::id,
                #table_ident::id,
            >;
            fn __autumn_upsert_set() -> Self::UpsertSet {
                use ::autumn_web::reexports::diesel::ExpressionMethods as _;
                #table_ident::id.eq(#table_ident::id)
            }
        }

        impl ::autumn_web::repository::AutumnUpsertExecutionExt for #name {
            type Model = Self;
            async fn __autumn_execute_upsert(
                chunk: &[Self::Model],
                tenant_id: ::core::option::Option<&str>,
                conn: &mut ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            ) -> ::core::result::Result<::std::vec::Vec<Self::Model>, ::autumn_web::reexports::diesel::result::Error> {
                Self::__autumn_execute_upsert(chunk, tenant_id, conn).await
            }
        }

        impl ::autumn_web::repository::AutumnCorrelateExt for #name {
            type NewModel = #new_name;
            fn __autumn_correlate_new(
                inputs: &[Self::NewModel],
                record: &Self,
                matched: &mut [bool],
            ) -> ::core::option::Option<usize> {
                Self::__autumn_correlate_new(inputs, record, matched)
            }

            fn __autumn_correlate_model(
                inputs: &[Self],
                record: &Self,
                matched: &mut [bool],
            ) -> ::core::option::Option<usize> {
                Self::__autumn_correlate_model(inputs, record, matched)
            }
        }


        impl #new_name {
            pub const __AUTUMN_COLUMN_COUNT: usize = #new_column_count;

            #[doc(hidden)]
            pub fn __autumn_column_count(&self) -> usize {
                Self::__AUTUMN_COLUMN_COUNT
            }
        }

        #can_set_tenant_id_impl
        #model_tenant_id_meta_impl

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #vis enum #field_enum_name {
            #(#field_enum_variants,)*
        }

        /// Extension trait providing `from_patch` and per-field `DraftField` accessors
        /// for `UpdateDraft<#name>`.
        ///
        /// Generated by `#[model]`. Import this trait to call `from_patch()` or
        /// field accessor methods on `UpdateDraft<#name>`.
        #vis trait #draft_ext_name {
            /// Build a draft by merging the current record with a patch.
            ///
            /// Returns `Err` if a non-nullable field has `Patch::Clear`.
            fn from_patch(current: &#name, patch: &#update_name) -> ::autumn_web::AutumnResult<::autumn_web::hooks::UpdateDraft<#name>>;

            #(#draft_accessor_sigs)*
        }

        impl #draft_ext_name for ::autumn_web::hooks::UpdateDraft<#name> {
            fn from_patch(current: &#name, patch: &#update_name) -> ::autumn_web::AutumnResult<Self> {
                let mut after = current.clone();
                #(#merge_arms)*
                Ok(Self::new_with_changes(current.clone(), after))
            }

            #(#draft_accessors)*
        }

        /// Factory builder for [`#name`].
        ///
        /// Produced by [`#name::factory()`]. All fields are pre-filled with
        /// `Default::default()` so callers only need to specify the fields that
        /// matter for their scenario.
        #[derive(Debug, Clone)]
        #vis struct #factory_name {
            #(#factory_struct_fields,)*
        }

        impl ::core::default::Default for #factory_name {
            fn default() -> Self {
                Self {
                    #(#factory_default_fields,)*
                }
            }
        }

        impl #factory_name {
            #(#factory_setters)*

            /// Build a [`#new_name`] instance from the current factory state.
            ///
            /// Does not touch the database. Use [`#factory_name::create`] to
            /// also persist the record.
            #[must_use]
            pub fn build(self) -> #new_name {
                #new_name {
                    #(#factory_build_fields,)*
                }
            }

            #factory_create_method
        }

        impl #name {
            /// Create a factory builder for constructing [`#name`] instances.
            ///
            /// Returns a [`#factory_name`] with all fields at their [`Default`]
            /// value. Override any subset with the fluent setter methods, then call
            /// `build()` for an in-memory instance or `create(pool)` to persist it.
            #[must_use]
            pub fn factory() -> #factory_name {
                #factory_name::default()
            }

            /// Returns the primary-key value of this model.
            ///
            /// Used by generated `#[factory_assoc]` code to extract the PK from a
            /// pre-built associated instance without hardcoding the field name.
            #[doc(hidden)]
            #[inline]
            pub fn __autumn_pk(&self) -> #pk_ty {
                self.#pk_id.clone()
            }
        }

        // ── Durable commit-hook codec ───────────────────────────────────
        // Hidden durable commit-hook codec. These methods serialize fields
        // individually so public serde visibility attributes do not drop
        // payload data needed by after_*_commit runners.
        impl #name {
            #[doc(hidden)]
            pub fn __autumn_commit_hook_to_value(
                &self,
            ) -> ::autumn_web::AutumnResult<::autumn_web::reexports::serde_json::Value>
            #commit_hook_serialize_where
            {
                let mut __autumn_object = ::autumn_web::reexports::serde_json::Map::new();
                #(#commit_hook_serialize_fields)*
                let mut __autumn_value =
                    ::autumn_web::reexports::serde_json::Value::Object(__autumn_object);
                // Encrypted columns must not be persisted in plaintext into the
                // durable `autumn_repository_commit_hooks` table (#805). Rewrite
                // them as recoverable ciphertext in their declared mode.
                #commit_hook_encrypt_stmt
                Ok(__autumn_value)
            }

            #[doc(hidden)]
            pub fn __autumn_commit_hook_from_value(
                __autumn_value: ::autumn_web::reexports::serde_json::Value,
            ) -> ::autumn_web::AutumnResult<Self>
            #commit_hook_deserialize_where
            {
                // Encrypted columns are persisted as ciphertext (see
                // `__autumn_commit_hook_to_value`); recover plaintext before the
                // model is reconstructed so replayed hooks see real values.
                let mut __autumn_decoded_value = __autumn_value;
                #commit_hook_decrypt_stmt
                let mut __autumn_object = match __autumn_decoded_value {
                    ::autumn_web::reexports::serde_json::Value::Object(__autumn_object) => __autumn_object,
                    __autumn_other => {
                        return Err(::autumn_web::AutumnError::internal_server_error_msg(format!(
                            "deserialize repository commit hook record for {}: expected object, got {}",
                            stringify!(#name),
                            __autumn_other
                        )));
                    }
                };
                #(#commit_hook_deserialize_fields)*
                Ok(Self {
                    #(#commit_hook_construct_fields,)*
                })
            }
        }

        // ── Optimistic-lock helpers ─────────────────────────────────────
        // Always emitted so the generated repository code can call them
        // unconditionally regardless of whether the model has a
        // `#[lock_version]` field. The `None` paths compile away with zero
        // overhead for models that don't use optimistic locking.
        impl #name {
            /// Returns the current stored lock version, or `None` if this model
            /// does not have a `#[lock_version]` field.
            #[doc(hidden)]
            #[inline]
            pub fn __autumn_lock_version_actual(&self) -> ::core::option::Option<i64> {
                #lock_version_actual_body
            }

            #etag_method
        }

        impl #update_name {
            /// Returns the client-supplied expected lock version, or `None`
            /// if this model does not have a `#[lock_version]` field.
            ///
            /// The repository compares this against the stored version and
            /// returns `RepositoryError::Conflict` on a mismatch.
            #[doc(hidden)]
            #[inline]
            pub fn __autumn_lock_version_expected(&self) -> ::core::option::Option<i64> {
                #lock_version_expected_body
            }
        }

        // ── OpenAPI schema impls ────────────────────────────────────────
        // Always emitted (OpenApiSchema is not feature-gated) so external
        // crates can register rich schemas without the openapi feature.

        impl ::autumn_web::openapi::OpenApiSchema for #name {
            fn schema_name() -> &'static str { stringify!(#name) }
            fn schema() -> ::serde_json::Value {
                #query_struct_schema_body
            }
        }

        impl ::autumn_web::openapi::OpenApiSchema for #new_name {
            fn schema_name() -> &'static str { stringify!(#new_name) }
            fn schema() -> ::serde_json::Value {
                #new_struct_schema_body
            }
        }

        impl ::autumn_web::openapi::OpenApiSchema for #update_name {
            fn schema_name() -> &'static str { stringify!(#update_name) }
            fn schema() -> ::serde_json::Value {
                #update_struct_schema_body
            }
        }

        impl ::autumn_web::repository::AutumnSearchableModel for #name {
            const IS_SEARCHABLE: bool = #is_searchable;
            const SEARCH_LANGUAGE: &'static str = #search_language;
            const SEARCH_FIELDS: &'static [(&'static str, char)] = &[
                #((#search_field_names, #search_field_weights)),*
            ];
        }

        // ── State machine impls (one per #[state_machine] field) ────────────
        #(#state_machine_impls)*
    }
}

pub fn infer_table_name(ident: &syn::Ident) -> String {
    let name = ident.to_string();
    let snake = pascal_to_snake(&name);
    format!("{snake}s")
}

pub fn pascal_to_snake(s: &str) -> String {
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

    // ── RED: #[lock_version] detection ────────────────────────────────────
    // These tests cover the new `excluded_from_new` behaviour (must also
    // exclude `#[lock_version]` fields) and the helper that detects whether
    // a field carries the attribute.

    #[test]
    fn lock_version_attr_detected_by_has_attr() {
        let field: syn::Field = syn::parse_quote! {
            #[lock_version]
            pub version: i32
        };
        assert!(has_attr(&field, "lock_version"));
    }

    #[test]
    fn lock_version_field_is_excluded_from_new() {
        let field: syn::Field = syn::parse_quote! {
            #[lock_version]
            pub lock_version: i32
        };
        // A #[lock_version] field must be absent from NewModel (the DB
        // supplies the initial value via a DEFAULT constraint).
        assert!(excluded_from_new(&field));
    }

    #[test]
    fn regular_field_is_not_excluded_from_new() {
        let field: syn::Field = syn::parse_quote! {
            pub title: String
        };
        assert!(!excluded_from_new(&field));
    }

    #[test]
    fn id_field_is_still_excluded_from_new() {
        let field: syn::Field = syn::parse_quote! {
            #[id]
            pub id: i64
        };
        assert!(excluded_from_new(&field));
    }

    #[test]
    fn encrypted_string_field_is_accepted() {
        let field: syn::Field = syn::parse_quote! {
            #[encrypted]
            pub token: String
        };
        assert!(validate_encrypted_field(&field).is_ok());
    }

    #[test]
    fn encrypted_plus_searchable_is_rejected() {
        // Search indexes the stored ciphertext, so plaintext queries would miss —
        // the combination must be a compile error (#805).
        let field: syn::Field = syn::parse_quote! {
            #[encrypted]
            #[searchable]
            pub token: String
        };
        let err = validate_encrypted_field(&field).unwrap_err();
        assert!(err.to_string().contains("searchable"));
    }

    #[test]
    fn lock_version_filtered_from_user_attrs() {
        let field: syn::Field = syn::parse_quote! {
            #[lock_version]
            pub version: i32
        };
        let attrs = user_attrs(&field);
        // The lock_version attribute must not leak onto the generated Diesel
        // struct — Diesel doesn't know about it and would emit a warning/error.
        assert!(attrs.is_empty());
    }

    #[test]
    fn model_commit_hook_codec_includes_serde_skipped_fields() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Account {
                    #[id]
                    pub id: i64,
                    pub email: String,
                    #[serde(skip_serializing)]
                    pub password_hash: String,
                    #[serde(skip)]
                    pub reset_token: Option<String>,
                }
            },
        );
        let generated = output.to_string();

        assert!(
            generated.contains("__autumn_commit_hook_to_value")
                && generated.contains("__autumn_commit_hook_from_value"),
            "models must implement the full-fidelity commit hook codec: {generated}"
        );
        assert!(
            generated.contains("\"password_hash\""),
            "commit hook codec must serialize skip_serializing fields: {generated}"
        );
        assert!(
            generated.contains("\"reset_token\""),
            "commit hook codec must serialize skip fields instead of defaulting them: {generated}"
        );
    }

    #[test]
    fn model_commit_hook_codec_preserves_serde_adapters() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct LedgerEntry {
                    #[id]
                    pub id: i64,
                    #[serde(with = "cents_adapter")]
                    pub amount_cents: i64,
                    #[serde(
                        serialize_with = "token_adapter::serialize",
                        deserialize_with = "token_adapter::deserialize"
                    )]
                    pub external_token: String,
                }
            },
        );
        let generated = output.to_string();

        assert!(
            generated.contains("__AutumnCommitHookSerializeField"),
            "commit hook codec must serialize adapted fields through serde field helpers: {generated}"
        );
        assert!(
            generated.contains("__AutumnCommitHookDeserializeField"),
            "commit hook codec must deserialize adapted fields through serde field helpers: {generated}"
        );
        assert!(
            generated.contains("with = \"cents_adapter\""),
            "commit hook codec must preserve serde with adapters: {generated}"
        );
        assert!(
            generated.contains("serialize_with = \"token_adapter::serialize\""),
            "commit hook codec must preserve serialize_with adapters: {generated}"
        );
        assert!(
            generated.contains("deserialize_with = \"token_adapter::deserialize\""),
            "commit hook codec must preserve deserialize_with adapters: {generated}"
        );
    }

    // ── Existing tests ────────────────────────────────────────────────────

    #[test]
    fn model_commit_hook_codec_defaults_missing_compatible_fields() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Account {
                    #[id]
                    pub id: i64,
                    pub reset_token: Option<String>,
                    #[serde(default = "default_reset_token")]
                    pub special_token: Option<String>,
                    #[serde(default)]
                    pub display_name: String,
                    #[serde(default = "default_status")]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();

        assert!(
            generated.contains(":: core :: option :: Option :: None"),
            "missing Option fields in old durable payloads should default to None: {generated}"
        );
        assert!(
            generated.contains(":: core :: default :: Default :: default ()"),
            "missing #[serde(default)] fields in old durable payloads should use Default::default(): {generated}"
        );
        assert!(
            generated.contains("default_status ()"),
            "missing #[serde(default = \"...\")] fields in old durable payloads should call the configured default function: {generated}"
        );
        assert!(
            generated.contains("default_reset_token ()"),
            "explicit serde defaults should beat the generic Option::None fallback: {generated}"
        );
    }

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
    fn pascal_case_simple() {
        assert_eq!(pascal_case("title"), "Title");
    }

    #[test]
    fn pascal_case_multi_word() {
        assert_eq!(pascal_case("approved_at"), "ApprovedAt");
    }

    #[test]
    fn pascal_case_single_char() {
        assert_eq!(pascal_case("x"), "X");
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

    // ── RED: etag() derivation from #[lock_version] ────────────────────────

    #[test]
    fn lock_version_model_emits_etag_method() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Post {
                    #[id]
                    pub id: i64,
                    pub title: String,
                    #[lock_version]
                    pub lock_version: i64,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("pub fn etag"),
            "model with #[lock_version] must emit `pub fn etag`: {generated}"
        );
    }

    #[test]
    fn model_without_lock_version_does_not_emit_etag_method() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Post {
                    #[id]
                    pub id: i64,
                    pub title: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            !generated.contains("pub fn etag"),
            "model without #[lock_version] must NOT emit `pub fn etag`: {generated}"
        );
    }

    #[test]
    fn etag_method_calls_into_etag_on_lock_version_field() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Post {
                    #[id]
                    pub id: i64,
                    pub title: String,
                    #[lock_version]
                    pub lock_version: i64,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("IntoETag") || generated.contains("into_etag"),
            "etag() must call IntoETag::into_etag on the lock_version field: {generated}"
        );
        assert!(
            generated.contains("lock_version"),
            "etag() method body must reference the lock_version field: {generated}"
        );
    }

    // ── RED: declarative state machines ───────────────────────────────────────
    // These tests define the expected generated API for `#[state_machine(...)]`
    // field attributes. All will fail until the feature is implemented.

    #[test]
    fn state_machine_emits_can_transition_method() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                        processing -> shipped,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("can_transition_status_to"),
            "#[state_machine] must emit `can_transition_status_to`: {generated}"
        );
    }

    #[test]
    fn state_machine_emits_transition_to_method() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("transition_status_to"),
            "#[state_machine] must emit `transition_status_to`: {generated}"
        );
    }

    #[test]
    fn state_machine_emits_transitions_constant() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                        processing -> shipped,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("__AUTUMN_SM_STATUS_TRANSITIONS"),
            "#[state_machine] must emit `__AUTUMN_SM_STATUS_TRANSITIONS` constant: {generated}"
        );
    }

    #[test]
    fn state_machine_transition_table_contains_from_to_pairs() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                        processing -> shipped,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("\"pending\"") && generated.contains("\"processing\""),
            "transition table must contain the from/to state strings: {generated}"
        );
        assert!(
            generated.contains("\"shipped\""),
            "transition table must contain all destination states: {generated}"
        );
    }

    #[test]
    fn state_machine_with_guard_calls_guard_method() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        processing -> shipped: "can_ship",
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("can_ship"),
            "guarded transition must call the guard method `can_ship`: {generated}"
        );
    }

    #[test]
    fn state_machine_guard_stored_in_transition_table() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        processing -> shipped: "can_ship",
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("Some (\"can_ship\")") || generated.contains("Some(\"can_ship\")"),
            "guarded transition must store the guard name in the transition table: {generated}"
        );
    }

    #[test]
    fn state_machine_unguarded_transition_table_entry_has_none() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("None"),
            "unguarded transition must store None in the transition table: {generated}"
        );
    }

    #[test]
    fn state_machine_transition_method_returns_autumn_result() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("AutumnResult"),
            "transition_*_to must return AutumnResult: {generated}"
        );
    }

    #[test]
    fn state_machine_attribute_not_leaked_to_diesel_struct() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(
                        pending -> processing,
                    ))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        // The `state_machine` attribute must not appear inside the Diesel struct
        // definition — Diesel doesn't know about it and would emit errors.
        // We check that it does NOT appear as a field-level #[state_machine].
        // The generated constant/methods may contain the word though.
        let struct_block = generated
            .find("pub struct Order")
            .map(|i| &generated[i..i + 500])
            .unwrap_or("");
        assert!(
            !struct_block.contains("# [state_machine]")
                && !struct_block.contains("#[state_machine]"),
            "`state_machine` attribute must not appear on the generated Diesel struct field: {struct_block}"
        );
    }

    #[test]
    fn state_machine_on_non_string_field_is_rejected() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(pending -> processing))]
                    pub amount: i64,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("only supported on `String` fields"),
            "#[state_machine] on a non-String field must emit a compile error: {generated}"
        );
    }

    #[test]
    fn state_machine_duplicate_attribute_on_same_field_is_rejected() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Order {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(pending -> processing))]
                    #[state_machine(transitions(processing -> shipped))]
                    pub status: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("multiple `#[state_machine]` attributes are not allowed"),
            "duplicate #[state_machine] on same field must emit a compile error: {generated}"
        );
    }

    #[test]
    fn state_machine_multiple_fields_emit_separate_methods() {
        let output = model_macro(
            TokenStream::new(),
            quote! {
                pub struct Ticket {
                    #[id]
                    pub id: i64,
                    #[state_machine(transitions(open -> in_progress, in_progress -> closed))]
                    pub status: String,
                    #[state_machine(transitions(low -> medium, medium -> high))]
                    pub priority: String,
                }
            },
        );
        let generated = output.to_string();
        assert!(
            generated.contains("can_transition_status_to"),
            "multi-sm model must emit `can_transition_status_to`: {generated}"
        );
        assert!(
            generated.contains("can_transition_priority_to"),
            "multi-sm model must emit `can_transition_priority_to`: {generated}"
        );
        assert!(
            generated.contains("transition_status_to"),
            "multi-sm model must emit `transition_status_to`: {generated}"
        );
        assert!(
            generated.contains("transition_priority_to"),
            "multi-sm model must emit `transition_priority_to`: {generated}"
        );
    }
}
