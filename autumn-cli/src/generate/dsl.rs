//! Field-type DSL parser for `autumn generate`.
//!
//! Turns command-line tokens like `title:String`, `tags:Vec<u8>`, or
//! `published:Option<bool>` into a structured [`Field`] that knows both its
//! Rust type (for the `#[model]` struct) and its SQL type (for the migration).

use super::GenerateError;

/// A single field parsed from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Column / struct field name (`snake_case`).
    pub name: String,
    /// Underlying type, ignoring `Option` wrapping.
    pub kind: FieldKind,
    /// True when the field was given as `Option<…>`.
    pub nullable: bool,
}

impl Field {
    /// The Rust type for the `#[model]` struct.
    #[must_use]
    pub fn rust_type(&self) -> String {
        let inner = self.kind.rust_type();
        if self.nullable {
            format!("Option<{inner}>")
        } else {
            inner.to_string()
        }
    }

    /// The Diesel `schema.rs` type token (always a single identifier).
    #[must_use]
    pub fn schema_type(&self) -> String {
        let inner = self.kind.schema_type();
        if self.nullable {
            format!("Nullable<{inner}>")
        } else {
            inner.to_string()
        }
    }

    /// The SQL column type, without nullability suffix.
    #[must_use]
    pub const fn sql_type(&self) -> &'static str {
        self.kind.sql_type()
    }

    /// `"NULL"` or `"NOT NULL"` to append in the migration.
    #[must_use]
    pub const fn sql_nullability(&self) -> &'static str {
        if self.nullable { "NULL" } else { "NOT NULL" }
    }
}

/// The supported field types. Mirrors the documented public surface in the
/// `autumn generate --help` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// `String` — `TEXT`.
    String,
    /// `Text` (alias for `String`) — `TEXT`.
    Text,
    /// `i32` — `INTEGER`.
    I32,
    /// `i64` — `BIGINT`.
    I64,
    /// `bool` — `BOOLEAN`.
    Bool,
    /// `f32` — `REAL`.
    F32,
    /// `f64` — `DOUBLE PRECISION`.
    F64,
    /// `Uuid` — `UUID`.
    Uuid,
    /// `NaiveDateTime` — `TIMESTAMP`.
    NaiveDateTime,
    /// `DateTime` — `TIMESTAMPTZ`.
    DateTime,
    /// `Vec<u8>` / `Bytea` — `BYTEA`.
    Bytea,
}

impl FieldKind {
    /// Rust type token used inside `#[model]` structs.
    #[must_use]
    pub const fn rust_type(self) -> &'static str {
        match self {
            Self::String | Self::Text => "String",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Bool => "bool",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Uuid => "uuid::Uuid",
            Self::NaiveDateTime => "chrono::NaiveDateTime",
            Self::DateTime => "chrono::DateTime<chrono::Utc>",
            Self::Bytea => "Vec<u8>",
        }
    }

    /// Diesel `table!` schema type token.
    #[must_use]
    pub const fn schema_type(self) -> &'static str {
        match self {
            Self::String | Self::Text => "Text",
            Self::I32 => "Int4",
            Self::I64 => "Int8",
            Self::Bool => "Bool",
            Self::F32 => "Float4",
            Self::F64 => "Float8",
            Self::Uuid => "Uuid",
            Self::NaiveDateTime => "Timestamp",
            Self::DateTime => "Timestamptz",
            Self::Bytea => "Bytea",
        }
    }

    /// `PostgreSQL` column type, without `NOT NULL` / `NULL`.
    #[must_use]
    pub const fn sql_type(self) -> &'static str {
        match self {
            Self::String | Self::Text => "TEXT",
            Self::I32 => "INTEGER",
            Self::I64 => "BIGINT",
            Self::Bool => "BOOLEAN",
            Self::F32 => "REAL",
            Self::F64 => "DOUBLE PRECISION",
            Self::Uuid => "UUID",
            Self::NaiveDateTime => "TIMESTAMP",
            Self::DateTime => "TIMESTAMPTZ",
            Self::Bytea => "BYTEA",
        }
    }
}

/// Comma-separated list of supported types, for error messages and `--help`.
pub const SUPPORTED_TYPES: &str = "String, Text, i32, i64, bool, f32, f64, \
    Uuid, NaiveDateTime, DateTime, Vec<u8>, Bytea, Option<…>";

/// Parse a single CLI token of the form `name:Type`.
///
/// # Errors
/// Returns [`GenerateError::InvalidField`] if the token is malformed or the
/// type is not in the supported set.
pub fn parse_field(token: &str) -> Result<Field, GenerateError> {
    let (name, ty) = token
        .split_once(':')
        .ok_or_else(|| GenerateError::InvalidField {
            token: token.to_owned(),
            reason: "expected `name:Type` (missing colon)".into(),
        })?;

    let name = name.trim();
    let ty = ty.trim();

    if name.is_empty() {
        return Err(GenerateError::InvalidField {
            token: token.to_owned(),
            reason: "field name is empty".into(),
        });
    }
    if !is_valid_ident(name) {
        return Err(GenerateError::InvalidField {
            token: token.to_owned(),
            reason: format!("'{name}' is not a valid snake_case identifier"),
        });
    }
    if is_rust_keyword(name) {
        return Err(GenerateError::InvalidField {
            token: token.to_owned(),
            reason: format!("'{name}' is a Rust keyword and cannot be used as a struct field name"),
        });
    }

    let (kind, nullable) = parse_type(ty).ok_or_else(|| GenerateError::InvalidField {
        token: token.to_owned(),
        reason: format!("unsupported type '{ty}'. Supported: {SUPPORTED_TYPES}"),
    })?;

    Ok(Field {
        name: name.to_owned(),
        kind,
        nullable,
    })
}

/// Parse a list of `name:Type` tokens.
///
/// # Errors
/// Bubbles up the first failed token, and rejects duplicate field names —
/// emitting two entries with the same column name would produce duplicate
/// struct members and duplicate SQL columns.
pub fn parse_fields(tokens: &[String]) -> Result<Vec<Field>, GenerateError> {
    let mut fields: Vec<Field> = Vec::with_capacity(tokens.len());
    for token in tokens {
        let field = parse_field(token)?;
        if let Some(prev) = fields.iter().find(|f| f.name == field.name) {
            return Err(GenerateError::InvalidField {
                token: token.clone(),
                reason: format!(
                    "duplicate field name '{name}' (previously declared as '{name}:{prev_ty}')",
                    name = field.name,
                    prev_ty = prev.rust_type()
                ),
            });
        }
        fields.push(field);
    }
    Ok(fields)
}

fn parse_type(ty: &str) -> Option<(FieldKind, bool)> {
    if let Some(inner) = strip_wrapper(ty, "Option") {
        let kind = atomic_type(inner.trim())?;
        Some((kind, true))
    } else {
        atomic_type(ty).map(|k| (k, false))
    }
}

fn atomic_type(ty: &str) -> Option<FieldKind> {
    match ty {
        "String" => Some(FieldKind::String),
        "Text" => Some(FieldKind::Text),
        "i32" => Some(FieldKind::I32),
        "i64" => Some(FieldKind::I64),
        "bool" => Some(FieldKind::Bool),
        "f32" => Some(FieldKind::F32),
        "f64" => Some(FieldKind::F64),
        "Uuid" => Some(FieldKind::Uuid),
        "NaiveDateTime" => Some(FieldKind::NaiveDateTime),
        "DateTime" => Some(FieldKind::DateTime),
        "Bytea" => Some(FieldKind::Bytea),
        _ => {
            // Allow `Vec<u8>` as a synonym for `Bytea`.
            strip_wrapper(ty, "Vec").and_then(|inner| {
                if inner.trim() == "u8" {
                    Some(FieldKind::Bytea)
                } else {
                    None
                }
            })
        }
    }
}

fn strip_wrapper<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let prefix = format!("{wrapper}<");
    let stripped = ty.strip_prefix(&prefix)?;
    stripped.strip_suffix('>')
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Strict and reserved Rust keywords that cannot appear as a struct field name
/// or module name without raw-identifier syntax. Rather than emitting `r#type:`
/// we reject the input so the generator never produces broken code.
///
/// Public so the resource-name validator in [`super::model`] can share the same
/// list.
pub(super) const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "do", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try",
    "type", "unsafe", "use", "where", "while", "yield", "abstract", "become", "box", "final",
    "macro", "override", "priv", "typeof", "unsized", "virtual",
];

pub(super) fn is_rust_keyword(s: &str) -> bool {
    RUST_KEYWORDS.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_string_field() {
        let f = parse_field("title:String").unwrap();
        assert_eq!(f.name, "title");
        assert_eq!(f.kind, FieldKind::String);
        assert!(!f.nullable);
        assert_eq!(f.rust_type(), "String");
        assert_eq!(f.sql_type(), "TEXT");
        assert_eq!(f.schema_type(), "Text");
    }

    #[test]
    fn parse_text_alias() {
        let f = parse_field("body:Text").unwrap();
        assert_eq!(f.kind, FieldKind::Text);
        assert_eq!(f.rust_type(), "String");
        assert_eq!(f.sql_type(), "TEXT");
    }

    #[test]
    fn parse_optional_field() {
        let f = parse_field("description:Option<String>").unwrap();
        assert_eq!(f.kind, FieldKind::String);
        assert!(f.nullable);
        assert_eq!(f.rust_type(), "Option<String>");
        assert_eq!(f.sql_nullability(), "NULL");
        assert_eq!(f.schema_type(), "Nullable<Text>");
    }

    #[test]
    fn parse_bytea_via_vec() {
        let f = parse_field("data:Vec<u8>").unwrap();
        assert_eq!(f.kind, FieldKind::Bytea);
        assert_eq!(f.rust_type(), "Vec<u8>");
        assert_eq!(f.sql_type(), "BYTEA");
    }

    #[test]
    fn parse_bytea_alias() {
        let f = parse_field("data:Bytea").unwrap();
        assert_eq!(f.kind, FieldKind::Bytea);
    }

    #[test]
    fn parse_uuid() {
        let f = parse_field("token:Uuid").unwrap();
        assert_eq!(f.rust_type(), "uuid::Uuid");
        assert_eq!(f.sql_type(), "UUID");
    }

    #[test]
    fn parse_datetime() {
        let f = parse_field("created_at:DateTime").unwrap();
        assert_eq!(f.rust_type(), "chrono::DateTime<chrono::Utc>");
        assert_eq!(f.schema_type(), "Timestamptz");
    }

    #[test]
    fn parse_naive_datetime() {
        let f = parse_field("created_at:NaiveDateTime").unwrap();
        assert_eq!(f.rust_type(), "chrono::NaiveDateTime");
        assert_eq!(f.schema_type(), "Timestamp");
    }

    #[test]
    fn parse_all_numeric_types() {
        assert_eq!(parse_field("a:i32").unwrap().sql_type(), "INTEGER");
        assert_eq!(parse_field("b:i64").unwrap().sql_type(), "BIGINT");
        assert_eq!(parse_field("c:f32").unwrap().sql_type(), "REAL");
        assert_eq!(parse_field("d:f64").unwrap().sql_type(), "DOUBLE PRECISION");
        assert_eq!(parse_field("e:bool").unwrap().sql_type(), "BOOLEAN");
    }

    #[test]
    fn unknown_type_rejected() {
        let err = parse_field("price:Decimal").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Decimal"));
        assert!(msg.contains("Supported:"));
    }

    #[test]
    fn missing_colon_rejected() {
        let err = parse_field("title").unwrap_err();
        assert!(err.to_string().contains("missing colon"));
    }

    #[test]
    fn empty_name_rejected() {
        let err = parse_field(":String").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn pascal_case_name_rejected() {
        let err = parse_field("Title:String").unwrap_err();
        assert!(err.to_string().contains("snake_case"));
    }

    #[test]
    fn rust_keyword_field_name_rejected() {
        // `pub type: String` would be a Rust syntax error.
        let err = parse_field("type:String").unwrap_err();
        assert!(err.to_string().contains("Rust keyword"));
    }

    #[test]
    fn other_keywords_also_rejected() {
        for kw in ["fn", "match", "struct", "self", "impl", "ref", "move"] {
            let token = format!("{kw}:String");
            assert!(
                parse_field(&token).is_err(),
                "expected '{kw}' to be rejected"
            );
        }
    }

    #[test]
    fn nested_option_is_unsupported() {
        // Option<Option<String>> is intentionally not part of the surface.
        let err = parse_field("x:Option<Option<String>>").unwrap_err();
        assert!(err.to_string().contains("unsupported type"));
    }

    #[test]
    fn vec_of_other_types_rejected() {
        let err = parse_field("xs:Vec<i32>").unwrap_err();
        assert!(err.to_string().contains("unsupported type"));
    }

    #[test]
    fn parse_multiple_fields() {
        let tokens = vec!["title:String".into(), "count:i64".into()];
        let fs = parse_fields(&tokens).unwrap();
        assert_eq!(fs.len(), 2);
        assert_eq!(fs[0].name, "title");
        assert_eq!(fs[1].name, "count");
    }

    #[test]
    fn duplicate_field_names_rejected() {
        // `title:String title:Text` would emit two `title` columns.
        let tokens = vec!["title:String".into(), "title:Text".into()];
        let err = parse_fields(&tokens).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate"),
            "expected duplicate error, got: {msg}"
        );
        assert!(msg.contains("title"));
    }

    #[test]
    fn whitespace_around_tokens_tolerated() {
        let f = parse_field(" name : String ").unwrap();
        assert_eq!(f.name, "name");
        assert_eq!(f.kind, FieldKind::String);
    }
}
