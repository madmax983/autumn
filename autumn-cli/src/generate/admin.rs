//! `autumn generate admin` — admin resource adapter for `autumn-admin-plugin`.
//!
//! For an existing `#[model]` (produced by `autumn generate model` or
//! `autumn generate scaffold`), emits:
//!
//! - `src/admin/<snake>.rs` — a full `AdminModel` implementation with list,
//!   detail, create, edit, delete, search, pagination, and bulk-delete.
//! - `tests/<snake>_admin.rs` — a request-level smoke test proving anonymous
//!   access is rejected and an admin user can load the list page.
//!
//! The generator derives safe field metadata from the supplied field tokens
//! and lets the user refine it through `--hidden`, `--readonly`, `--password`,
//! `--select`, and `--exclude` flags.

use std::path::Path;

use super::dsl::{Field, FieldKind, parse_fields};
use super::emit::Plan;
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::add_mod_declaration;
use super::{Flags, GenerateError, ensure_project_root};

/// A parsed `--select FIELD=val1,val2,...` spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectSpec {
    /// Field name.
    pub field: String,
    /// Ordered list of option values (labels are humanized from values).
    pub values: Vec<String>,
}

/// Parse `--select` tokens of the form `field=val1,val2,...` or `field`.
///
/// The bare `field` form (no `=`) emits a `Select(vec![])` placeholder so the
/// user can fill in options without editing generated internals.
///
/// # Errors
/// Returns [`GenerateError::InvalidField`] if the token is blank.
pub fn parse_select_specs(tokens: &[String]) -> Result<Vec<SelectSpec>, GenerateError> {
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let (field, values) = match token.split_once('=') {
            Some((f, v)) => (
                f.trim().to_owned(),
                v.split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>(),
            ),
            None => (token.trim().to_owned(), vec![]),
        };
        if field.is_empty() {
            return Err(GenerateError::InvalidField {
                token: token.clone(),
                reason: "field name is empty in --select spec".into(),
            });
        }
        out.push(SelectSpec { field, values });
    }
    Ok(out)
}

/// Options specific to `autumn generate admin`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminOptions {
    /// Field names to render as `AdminFieldKind::Hidden`.
    pub hidden: Vec<String>,
    /// Field names to mark as read-only (`.readonly()`).
    pub readonly: Vec<String>,
    /// Field names to render as `AdminFieldKind::Password`.
    pub password: Vec<String>,
    /// Fields to render as `AdminFieldKind::Select` with fixed option values.
    /// Each entry is a [`SelectSpec`] parsed from a `field=val1,val2,...` token.
    pub select: Vec<SelectSpec>,
    /// Field names to omit from the generated `fields()` list entirely.
    pub exclude: Vec<String>,
}

/// Compute the file actions for `autumn generate admin` with default options.
///
/// # Errors
/// Returns [`GenerateError`] if the project layout is invalid, the name is
/// malformed, a field token cannot be parsed, or the target model file does
/// not exist.
#[cfg(test)]
pub fn plan_admin(
    project_root: &Path,
    name: &str,
    field_tokens: &[String],
) -> Result<Plan, GenerateError> {
    plan_admin_with_options(project_root, name, field_tokens, &AdminOptions::default())
}

/// Compute the file actions for `autumn generate admin`, with full options.
///
/// # Errors
/// Same as [`plan_admin`], plus options-level errors (unknown field names).
pub fn plan_admin_with_options(
    project_root: &Path,
    name: &str,
    field_tokens: &[String],
    options: &AdminOptions,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;

    let snake_name = snake(name);
    let pascal_name = pascal(name);

    // Verify the source model file exists before emitting anything.
    let model_path = project_root
        .join("src")
        .join("models")
        .join(format!("{snake_name}.rs"));
    if !model_path.exists() {
        return Err(GenerateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "model file not found: {}; run `autumn generate model {pascal_name} …` first, \
                 or `autumn generate scaffold {pascal_name} …` to include a repository",
                model_path.display()
            ),
        )));
    }

    let fields = parse_fields(field_tokens)?;
    let plural = pluralize(&snake_name);
    let plural_pascal = pascal(&plural);

    let mut plan = Plan::new(project_root);

    // Admin adapter: `src/admin/<snake>.rs`
    let admin_dir = project_root.join("src").join("admin");
    plan.create(
        admin_dir.join(format!("{snake_name}.rs")),
        render_admin_file(
            &pascal_name,
            &snake_name,
            &plural,
            &plural_pascal,
            &fields,
            options,
        ),
    );

    // Admin module declaration: `src/admin/mod.rs`
    let admin_mod_path = admin_dir.join("mod.rs");
    plan.modify(
        admin_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&admin_mod_path), &snake_name),
    );

    // Smoke test: `tests/<snake>_admin.rs`
    plan.create(
        project_root
            .join("tests")
            .join(format!("{snake_name}_admin.rs")),
        render_admin_smoke_test(&pascal_name, &snake_name, &plural, &fields, options),
    );

    Ok(plan)
}

/// CLI entry point for `autumn generate admin`.
pub fn run(name: &str, field_tokens: &[String], flags: Flags, options: &AdminOptions) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let plan = plan_admin_with_options(&cwd, name, field_tokens, options);
    match plan.and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

// ── Field metadata derivation ────────────────────────────────────────────────

const fn admin_field_kind(field: &Field) -> &'static str {
    match field.kind {
        FieldKind::String | FieldKind::Uuid => "AdminFieldKind::Text",
        FieldKind::Text => "AdminFieldKind::TextArea",
        FieldKind::I32 | FieldKind::I64 => "AdminFieldKind::Integer",
        FieldKind::Bool => "AdminFieldKind::Boolean",
        FieldKind::F32 | FieldKind::F64 => "AdminFieldKind::Float",
        FieldKind::NaiveDateTime | FieldKind::DateTime => "AdminFieldKind::DateTime",
        FieldKind::Bytea => "AdminFieldKind::Hidden",
    }
}

const fn is_default_searchable(field: &Field) -> bool {
    matches!(field.kind, FieldKind::String | FieldKind::Text)
}

const fn is_default_filterable(field: &Field) -> bool {
    matches!(field.kind, FieldKind::Bool)
}

const fn is_default_readonly(field: &Field) -> bool {
    matches!(
        field.kind,
        FieldKind::NaiveDateTime | FieldKind::DateTime | FieldKind::Uuid
    )
}

const fn is_default_optional(field: &Field) -> bool {
    field.nullable || matches!(field.kind, FieldKind::NaiveDateTime | FieldKind::DateTime)
}

fn is_default_hide_from_list(field: &Field) -> bool {
    // TextArea bodies and binary blobs clutter the table; updated_at is redundant noise.
    matches!(field.kind, FieldKind::Text | FieldKind::Bytea) || field.name == "updated_at"
}

fn is_update_writable(field: &Field, options: &AdminOptions) -> bool {
    !options.exclude.contains(&field.name)
        && !options.hidden.contains(&field.name)
        && !options.readonly.contains(&field.name)
        && !is_default_readonly(field)
        && !matches!(field.kind, FieldKind::Bytea)
}

// ── Template rendering ───────────────────────────────────────────────────────

#[allow(
    clippy::too_many_lines,
    reason = "Single template function — splitting produces less readable output, not more."
)]
fn render_admin_file(
    pascal_name: &str,
    snake_name: &str,
    plural: &str,
    plural_pascal: &str,
    fields: &[Field],
    options: &AdminOptions,
) -> String {
    let fields_vec = render_fields_vec(fields, options);
    let apply_filters = render_apply_filters(plural, fields, options);
    let apply_sort = render_apply_sort(plural, fields, options);
    let update_body = render_update_body(pascal_name, snake_name, plural, fields, options);

    format!(
        r#"//! Generated by `autumn generate admin`.
//!
//! Implements `AdminModel` for `{pascal_name}` using `autumn-admin-plugin`.
//! Edit freely — once generated, this is ordinary user code.

use autumn_admin_plugin::prelude::*;
use autumn_web::Patch;
use diesel::OptionalExtension;
use diesel::prelude::*;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::deadpool::Pool;
use serde_json::Value;

use crate::models::{snake_name}::{{{pascal_name}, New{pascal_name}, Update{pascal_name}}};
use crate::schema::{plural};

#[derive(Clone, Copy, Default)]
pub struct {pascal_name}Admin;

impl {pascal_name}Admin {{
    fn pool_error(e: impl std::fmt::Display) -> AdminError {{
        AdminError::Database(e.to_string())
    }}
    fn validation_error(e: impl std::fmt::Display) -> AdminError {{
        AdminError::Validation(e.to_string())
    }}
    fn other_error(e: impl std::fmt::Display) -> AdminError {{
        AdminError::Other(e.to_string())
    }}
    fn serialize_{snake_name}(row: {pascal_name}) -> Result<Value, AdminError> {{
        serde_json::to_value(row).map_err(Self::other_error)
    }}

{apply_filters}

{apply_sort}
}}

impl AdminModel for {pascal_name}Admin {{
    fn slug(&self) -> &'static str {{
        "{plural}"
    }}

    fn display_name(&self) -> &'static str {{
        "{pascal_name}"
    }}

    fn display_name_plural(&self) -> &'static str {{
        "{plural_pascal}"
    }}

    fn fields(&self) -> Vec<AdminField> {{
        vec![
            AdminField::new("id", AdminFieldKind::Hidden).readonly().hide_from_list(),
{fields_vec}        ]
    }}

    fn list(
        &self,
        pool: &Pool<AsyncPgConnection>,
        params: ListParams,
    ) -> AdminFuture<'_, ListResult> {{
        let pool = pool.clone();
        Box::pin(async move {{
            let mut conn = pool.get().await.map_err(Self::pool_error)?;

            let total: i64 = Self::apply_filters({plural}::table.into_boxed(), &params)
                .count()
                .get_result(&mut conn)
                .await
                .map_err(Self::pool_error)?;

            let mut query = Self::apply_sort(
                Self::apply_filters({plural}::table.into_boxed(), &params),
                &params,
            );
            if params.per_page > 0 {{
                let offset = params
                    .page
                    .saturating_sub(1)
                    .saturating_mul(params.per_page);
                query = query.limit(params.per_page as i64).offset(offset as i64);
            }}

            let records = query
                .select({pascal_name}::as_select())
                .load::<{pascal_name}>(&mut conn)
                .await
                .map_err(Self::pool_error)?
                .into_iter()
                .map(Self::serialize_{snake_name})
                .collect::<Result<Vec<_>, _>>()?;

            Ok(ListResult {{
                records,
                total: total as u64,
                page: params.page,
                per_page: params.per_page,
            }})
        }})
    }}

    fn get(&self, pool: &Pool<AsyncPgConnection>, id: i64) -> AdminFuture<'_, Option<Value>> {{
        let pool = pool.clone();
        Box::pin(async move {{
            let mut conn = pool.get().await.map_err(Self::pool_error)?;
            let row = {plural}::table
                .find(id)
                .select({pascal_name}::as_select())
                .first::<{pascal_name}>(&mut conn)
                .await
                .optional()
                .map_err(Self::pool_error)?;
            row.map(Self::serialize_{snake_name}).transpose()
        }})
    }}

    fn create(&self, pool: &Pool<AsyncPgConnection>, data: Value) -> AdminFuture<'_, Value> {{
        let pool = pool.clone();
        Box::pin(async move {{
            let new_row: New{pascal_name} =
                serde_json::from_value(data).map_err(Self::validation_error)?;
            let mut conn = pool.get().await.map_err(Self::pool_error)?;
            let created = diesel::insert_into({plural}::table)
                .values(&new_row)
                .returning({pascal_name}::as_returning())
                .get_result::<{pascal_name}>(&mut conn)
                .await
                .map_err(Self::pool_error)?;
            Self::serialize_{snake_name}(created)
        }})
    }}

    fn update(
        &self,
        pool: &Pool<AsyncPgConnection>,
        id: i64,
        data: Value,
    ) -> AdminFuture<'_, Value> {{
        let pool = pool.clone();
        Box::pin(async move {{
{update_body}
        }})
    }}

    fn delete(&self, pool: &Pool<AsyncPgConnection>, id: i64) -> AdminFuture<'_, ()> {{
        let pool = pool.clone();
        Box::pin(async move {{
            let mut conn = pool.get().await.map_err(Self::pool_error)?;
            let deleted = diesel::delete({plural}::table.find(id))
                .execute(&mut conn)
                .await
                .map_err(Self::pool_error)?;
            if deleted == 0 {{
                return Err(AdminError::NotFound);
            }}
            Ok(())
        }})
    }}
}}
"#
    )
}

fn render_select_kind(spec: &SelectSpec) -> String {
    if spec.values.is_empty() {
        return "AdminFieldKind::Select(vec![])".into();
    }
    let opts = spec
        .values
        .iter()
        .map(|v| {
            let label = v
                .split(['_', '-'])
                .map(|word| {
                    let mut chars = word.chars();
                    chars.next().map_or_else(String::new, |c| {
                        c.to_uppercase().collect::<String>() + chars.as_str()
                    })
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("SelectOption {{ value: \"{v}\".into(), label: \"{label}\".into() }}",)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("AdminFieldKind::Select(vec![{opts}])")
}

fn render_fields_vec(fields: &[Field], options: &AdminOptions) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for f in fields {
        if options.exclude.contains(&f.name) {
            continue;
        }
        let select_spec = options.select.iter().find(|s| s.field == f.name);
        let kind_str: String =
            if options.hidden.contains(&f.name) || matches!(f.kind, FieldKind::Bytea) {
                "AdminFieldKind::Hidden".into()
            } else if options.password.contains(&f.name) {
                "AdminFieldKind::Password".into()
            } else if let Some(spec) = select_spec {
                render_select_kind(spec)
            } else {
                admin_field_kind(f).into()
            };
        let _ = write!(
            out,
            "            AdminField::new(\"{name}\", {kind_str})",
            name = f.name
        );
        if options.readonly.contains(&f.name) || is_default_readonly(f) {
            let _ = write!(out, "\n                .readonly()");
        }
        // Select and hidden fields are not text-searchable.
        let is_select_or_hidden = select_spec.is_some() || options.hidden.contains(&f.name);
        if is_default_searchable(f) && !is_select_or_hidden {
            let _ = write!(out, "\n                .searchable()");
        }
        if is_default_filterable(f) && !is_select_or_hidden {
            let _ = write!(out, "\n                .filterable()");
        }
        if is_default_optional(f) {
            let _ = write!(out, "\n                .optional()");
        }
        if is_default_hide_from_list(f) {
            let _ = write!(out, "\n                .hide_from_list()");
        }
        out.push_str(",\n");
    }
    out
}

fn render_apply_filters(plural: &str, fields: &[Field], options: &AdminOptions) -> String {
    let searchable: Vec<&Field> = fields
        .iter()
        .filter(|f| !options.exclude.contains(&f.name))
        .filter(|f| is_default_searchable(f))
        .collect();

    let filterable_bool: Vec<&Field> = fields
        .iter()
        .filter(|f| !options.exclude.contains(&f.name))
        .filter(|f| is_default_filterable(f))
        .collect();

    let has_search = !searchable.is_empty();
    let has_filters = !filterable_bool.is_empty();

    // Decide whether `query` and `params` need to be mutable / used.
    let query_param = if has_search || has_filters {
        "mut query"
    } else {
        "query"
    };
    let params_prefix = if has_search || has_filters { "" } else { "_" };

    let search_block = if has_search {
        let conditions = searchable
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if i == 0 {
                    format!("{plural}::{}.ilike(pattern.clone())", f.name)
                } else {
                    format!(".or({plural}::{}.ilike(pattern.clone()))", f.name)
                }
            })
            .collect::<Vec<_>>()
            .join("\n                    ");
        format!(
            "        if let Some(search) = params.search.as_deref() {{\n\
             \t\t\tlet pattern = format!(\"%{{search}}%\");\n\
             \t\t\tquery = query.filter(\n\
             \t\t\t\t{conditions}\n\
             \t\t\t);\n\
             \t\t}}\n"
        )
    } else {
        String::new()
    };

    let filter_block = if has_filters {
        use std::fmt::Write;
        let mut arms = String::new();
        for f in &filterable_bool {
            let _ = write!(
                arms,
                "                \"{name}\" => match value.as_str() {{\n\
                 \t\t\t\t\t\"true\" | \"1\" | \"yes\" => \
                     query = query.filter({plural}::{name}.eq(true)),\n\
                 \t\t\t\t\t\"false\" | \"0\" | \"no\" => \
                     query = query.filter({plural}::{name}.eq(false)),\n\
                 \t\t\t\t\t_ => {{}}\n\
                 \t\t\t\t}},\n",
                name = f.name
            );
        }
        format!(
            "        for (name, value) in &params.filters {{\n\
             \t\t\tmatch name.as_str() {{\n\
             {arms}\
             \t\t\t\t_ => {{}}\n\
             \t\t\t}}\n\
             \t\t}}\n"
        )
    } else {
        String::new()
    };

    format!(
        "    fn apply_filters<'a>(\n\
         \t\t{query_param}: {plural}::BoxedQuery<'a, diesel::pg::Pg>,\n\
         \t\t{params_prefix}params: &'a ListParams,\n\
         \t) -> {plural}::BoxedQuery<'a, diesel::pg::Pg> {{\n\
         {search_block}\
         {filter_block}\
         \t\tquery\n\
         \t}}"
    )
}

fn render_apply_sort(plural: &str, fields: &[Field], options: &AdminOptions) -> String {
    use std::fmt::Write;
    let sortable: Vec<&Field> = fields
        .iter()
        .filter(|f| !options.exclude.contains(&f.name))
        .collect();

    let mut arms = String::new();
    // id arms always first
    let _ = write!(
        arms,
        "            (Some(\"id\"), SortDirection::Asc) => \
             query = query.order({plural}::id.asc()),\n\
         \t\t\t(Some(\"id\"), SortDirection::Desc) => \
             query = query.order({plural}::id.desc()),\n"
    );
    for f in &sortable {
        let _ = write!(
            arms,
            "            (Some(\"{name}\"), SortDirection::Asc) => \
                 query = query.order({plural}::{name}.asc()),\n\
             \t\t\t(Some(\"{name}\"), SortDirection::Desc) => \
                 query = query.order({plural}::{name}.desc()),\n",
            name = f.name
        );
    }
    // Default fallback
    let _ = write!(
        arms,
        "            (_, SortDirection::Asc) => query = query.order({plural}::id.asc()),\n\
         \t\t\t_ => query = query.order({plural}::id.desc()),\n"
    );

    format!(
        "    fn apply_sort<'a>(\n\
         \t\tmut query: {plural}::BoxedQuery<'a, diesel::pg::Pg>,\n\
         \t\tparams: &ListParams,\n\
         \t) -> {plural}::BoxedQuery<'a, diesel::pg::Pg> {{\n\
         \t\t\tmatch (params.sort_by.as_deref(), params.sort_dir) {{\n\
         {arms}\
         \t\t\t}}\n\
         \t\tquery\n\
         \t}}"
    )
}

fn render_update_body(
    pascal_name: &str,
    snake_name: &str,
    plural: &str,
    fields: &[Field],
    options: &AdminOptions,
) -> String {
    use std::fmt::Write;
    let writable: Vec<&Field> = fields
        .iter()
        .filter(|f| is_update_writable(f, options))
        .collect();

    let mut changes_fields = String::new();
    for f in &writable {
        let _ = writeln!(
            changes_fields,
            "                {name}: Patch::Set(new_row.{name}),",
            name = f.name
        );
    }

    format!(
        "            let new_row: New{pascal_name} =\n\
         \t\t\t\tserde_json::from_value(data).map_err(Self::validation_error)?;\n\
         \t\t\tlet changes = Update{pascal_name} {{\n\
         {changes_fields}\
         \t\t\t}};\n\
         \t\t\tlet diesel_changeset = changes.__to_changeset();\n\
         \t\t\tlet mut conn = pool.get().await.map_err(Self::pool_error)?;\n\
         \t\t\tlet updated = diesel::update({plural}::table.find(id))\n\
         \t\t\t\t.set(&diesel_changeset)\n\
         \t\t\t\t.returning({pascal_name}::as_returning())\n\
         \t\t\t\t.get_result::<{pascal_name}>(&mut conn)\n\
         \t\t\t\t.await\n\
         \t\t\t\t.optional()\n\
         \t\t\t\t.map_err(Self::pool_error)?;\n\
         \t\t\tupdated\n\
         \t\t\t\t.ok_or(AdminError::NotFound)\n\
         \t\t\t\t.and_then(Self::serialize_{snake_name})"
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Single template function — splitting produces less readable output, not more."
)]
fn render_admin_smoke_test(
    pascal_name: &str,
    snake_name: &str,
    plural: &str,
    fields: &[Field],
    options: &AdminOptions,
) -> String {
    // Build a minimal POST body for the create test using the writable fields.
    let writable: Vec<&Field> = fields
        .iter()
        .filter(|f| is_update_writable(f, options))
        .collect();

    let sample_form_body = if writable.is_empty() {
        String::new()
    } else {
        writable
            .iter()
            .map(|f| format!("{}=test", f.name))
            .collect::<Vec<_>>()
            .join("&")
    };
    let content_length = sample_form_body.len();

    format!(
        "//! Smoke tests generated by `autumn generate admin`.\n\
         //!\n\
         //! These tests require a running Autumn server. Set the environment variables\n\
         //! below and run `cargo test` against a live instance.\n\
         //!\n\
         //!   AUTUMN_TEST_BASE_URL=http://localhost:3000\n\
         //!   AUTUMN_TEST_ADMIN_SESSION=<session_cookie_value>\n\
         //!\n\
         //! If your app enables CSRF protection, configure `autumn.toml` to skip\n\
         //! CSRF in the `test` profile, or provide a valid token in the POST body.\n\
         \n\
         use std::io::{{Read, Write}};\n\
         \n\
         fn connect(base: &str) -> (std::net::TcpStream, String) {{\n\
         \tlet host_port = base\n\
         \t\t.trim_start_matches(\"http://\")\n\
         \t\t.trim_start_matches(\"https://\");\n\
         \t(\n\
         \t\tstd::net::TcpStream::connect(host_port)\n\
         \t\t\t.unwrap_or_else(|_| panic!(\"could not connect to {{base}}\")),\n\
         \t\thost_port.to_owned(),\n\
         \t)\n\
         }}\n\
         \n\
         fn read_response(stream: &mut std::net::TcpStream) -> String {{\n\
         \tlet mut response = String::new();\n\
         \tstream.read_to_string(&mut response).expect(\"read failed\");\n\
         \tresponse\n\
         }}\n\
         \n\
         /// Anonymous GET `/admin/{plural}` must be rejected — 302, 401, or 403.\n\
         #[test]\n\
         fn {snake_name}_admin_anonymous_access_is_rejected() {{\n\
         \tlet Ok(base) = std::env::var(\"AUTUMN_TEST_BASE_URL\") else {{\n\
         \t\teprintln!(\"skipping: AUTUMN_TEST_BASE_URL not set\");\n\
         \t\treturn;\n\
         \t}};\n\
         \tlet base = base.trim_end_matches('/');\n\
         \tlet (mut stream, host_port) = connect(base);\n\
         \tstream\n\
         \t\t.write_all(\n\
         \t\t\tformat!(\n\
         \t\t\t\t\"GET /admin/{plural} HTTP/1.1\\r\\nHost: {{host_port}}\\r\\nConnection: close\\r\\n\\r\\n\"\n\
         \t\t\t)\n\
         \t\t\t.as_bytes(),\n\
         \t\t)\n\
         \t\t.expect(\"write failed\");\n\
         \tlet response = read_response(&mut stream);\n\
         \tlet status_line = response.lines().next().unwrap_or(\"\");\n\
         \tassert!(\n\
         \t\tstatus_line.contains(\" 302\")\n\
         \t\t\t|| status_line.contains(\" 401\")\n\
         \t\t\t|| status_line.contains(\" 403\"),\n\
         \t\t\"{pascal_name} admin list should reject anonymous access, got: {{status_line}}\"\n\
         \t);\n\
         }}\n\
         \n\
         /// Admin GET `/admin/{plural}` with a valid session must return 200.\n\
         #[test]\n\
         fn {snake_name}_admin_list_loads_for_admin_user() {{\n\
         \tlet Ok(base) = std::env::var(\"AUTUMN_TEST_BASE_URL\") else {{\n\
         \t\teprintln!(\"skipping: AUTUMN_TEST_BASE_URL not set\");\n\
         \t\treturn;\n\
         \t}};\n\
         \tlet Ok(session_cookie) = std::env::var(\"AUTUMN_TEST_ADMIN_SESSION\") else {{\n\
         \t\teprintln!(\"skipping: AUTUMN_TEST_ADMIN_SESSION not set\");\n\
         \t\treturn;\n\
         \t}};\n\
         \tlet base = base.trim_end_matches('/');\n\
         \tlet (mut stream, host_port) = connect(base);\n\
         \tstream\n\
         \t\t.write_all(\n\
         \t\t\tformat!(\n\
         \t\t\t\t\"GET /admin/{plural} HTTP/1.1\\r\\n\\\n\
         \t\t\t\t Host: {{host_port}}\\r\\n\\\n\
         \t\t\t\t Cookie: {{session_cookie}}\\r\\n\\\n\
         \t\t\t\t Connection: close\\r\\n\\r\\n\"\n\
         \t\t\t)\n\
         \t\t\t.as_bytes(),\n\
         \t\t)\n\
         \t\t.expect(\"write failed\");\n\
         \tlet response = read_response(&mut stream);\n\
         \tassert!(\n\
         \t\tresponse.starts_with(\"HTTP/1.1 200\") || response.starts_with(\"HTTP/1.0 200\"),\n\
         \t\t\"{pascal_name} admin list did not return 200:\\n{{response}}\"\n\
         \t);\n\
         }}\n\
         \n\
         /// Admin POST `/admin/{plural}` with a valid session creates a record and\n\
         /// redirects (303) on success.\n\
         #[test]\n\
         fn {snake_name}_admin_create_redirects_for_admin_user() {{\n\
         \tlet Ok(base) = std::env::var(\"AUTUMN_TEST_BASE_URL\") else {{\n\
         \t\teprintln!(\"skipping: AUTUMN_TEST_BASE_URL not set\");\n\
         \t\treturn;\n\
         \t}};\n\
         \tlet Ok(session_cookie) = std::env::var(\"AUTUMN_TEST_ADMIN_SESSION\") else {{\n\
         \t\teprintln!(\"skipping: AUTUMN_TEST_ADMIN_SESSION not set\");\n\
         \t\treturn;\n\
         \t}};\n\
         \tlet base = base.trim_end_matches('/');\n\
         \tlet body = \"{sample_form_body}\";\n\
         \tlet (mut stream, host_port) = connect(base);\n\
         \tstream\n\
         \t\t.write_all(\n\
         \t\t\tformat!(\n\
         \t\t\t\t\"POST /admin/{plural} HTTP/1.1\\r\\n\\\n\
         \t\t\t\t Host: {{host_port}}\\r\\n\\\n\
         \t\t\t\t Cookie: {{session_cookie}}\\r\\n\\\n\
         \t\t\t\t Content-Type: application/x-www-form-urlencoded\\r\\n\\\n\
         \t\t\t\t Content-Length: {content_length}\\r\\n\\\n\
         \t\t\t\t Connection: close\\r\\n\\r\\n\\\n\
         \t\t\t\t {{body}}\"\n\
         \t\t\t)\n\
         \t\t\t.as_bytes(),\n\
         \t\t)\n\
         \t\t.expect(\"write failed\");\n\
         \tlet response = read_response(&mut stream);\n\
         \tlet status_line = response.lines().next().unwrap_or(\"\");\n\
         \tassert!(\n\
         \t\tstatus_line.contains(\" 200\")\n\
         \t\t\t|| status_line.contains(\" 302\")\n\
         \t\t\t|| status_line.contains(\" 303\"),\n\
         \t\t\"{pascal_name} admin create did not redirect on success:\\n{{status_line}}\"\n\
         \t);\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal project root: Cargo.toml + src/models/<snake>.rs.
    fn project_with_model(snake: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let models_dir = tmp.path().join("src/models");
        fs::create_dir_all(&models_dir).unwrap();
        // Touch the model file so the generator can find it.
        fs::write(models_dir.join(format!("{snake}.rs")), "// stub\n").unwrap();
        tmp
    }

    // ── RED phase ──────────────────────────────────────────────────────────

    #[test]
    fn plan_admin_fails_when_not_in_project() {
        let tmp = TempDir::new().unwrap();
        let err = plan_admin(tmp.path(), "Post", &[]).unwrap_err();
        assert!(
            matches!(err, GenerateError::NotInProject),
            "expected NotInProject, got {err:?}"
        );
    }

    #[test]
    fn plan_admin_fails_when_model_missing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let err = plan_admin(tmp.path(), "Post", &[]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("model file not found"),
            "expected 'model file not found' in error, got: {msg}"
        );
        assert!(
            msg.contains("post.rs"),
            "error should name the missing file, got: {msg}"
        );
        assert!(
            msg.contains("autumn generate model Post"),
            "error should suggest the fix, got: {msg}"
        );
    }

    #[test]
    fn plan_admin_creates_three_actions() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "body:Text".into(),
                "published:bool".into(),
            ],
        )
        .unwrap();

        let paths: Vec<String> = plan
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();

        for expected in [
            "src/admin/post.rs",
            "src/admin/mod.rs",
            "tests/post_admin.rs",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "missing expected action for {expected}; got {paths:?}"
            );
        }
        assert_eq!(
            plan.actions.len(),
            3,
            "expected exactly 3 actions, got {paths:?}"
        );
    }

    #[test]
    fn plan_admin_accepts_pascal_or_snake_name() {
        let tmp = project_with_model("blog_post");
        // Both "blog_post" and "BlogPost" should find src/models/blog_post.rs
        let plan_snake = plan_admin(tmp.path(), "blog_post", &[]).unwrap();
        let plan_pascal = plan_admin(tmp.path(), "BlogPost", &[]).unwrap();

        // Both produce the same set of paths.
        let paths_snake: Vec<_> = plan_snake
            .actions
            .iter()
            .map(|a| a.path().to_path_buf())
            .collect();
        let paths_pascal: Vec<_> = plan_pascal
            .actions
            .iter()
            .map(|a| a.path().to_path_buf())
            .collect();
        assert_eq!(paths_snake, paths_pascal);
    }

    #[test]
    fn plan_admin_rejects_invalid_field_tokens() {
        let tmp = project_with_model("post");
        let err = plan_admin(tmp.path(), "Post", &["title".into()]).unwrap_err();
        assert!(
            matches!(err, GenerateError::InvalidField { .. }),
            "expected InvalidField, got {err:?}"
        );
    }

    // ── GREEN phase (content verification) ────────────────────────────────

    #[test]
    fn execute_writes_admin_file_with_struct_and_impl() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "body:Text".into(),
                "published:bool".into(),
            ],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();

        // Struct + AdminModel impl
        assert!(admin.contains("pub struct PostAdmin;"));
        assert!(admin.contains("impl AdminModel for PostAdmin {"));

        // Slug, display names
        assert!(admin.contains("\"posts\""), "slug must be 'posts'");
        assert!(admin.contains("\"Post\""), "display_name must be 'Post'");
        assert!(
            admin.contains("\"Posts\""),
            "display_name_plural must be 'Posts'"
        );

        // Id is always hidden + readonly + hidden from list
        assert!(admin.contains("AdminField::new(\"id\", AdminFieldKind::Hidden)"));
        assert!(admin.contains(".readonly()"));
        assert!(admin.contains(".hide_from_list()"));

        // User fields
        assert!(
            admin.contains("AdminField::new(\"title\", AdminFieldKind::Text)"),
            "title should map to Text"
        );
        assert!(
            admin.contains("AdminField::new(\"body\", AdminFieldKind::TextArea)"),
            "body should map to TextArea"
        );
        assert!(
            admin.contains("AdminField::new(\"published\", AdminFieldKind::Boolean)"),
            "published should map to Boolean"
        );

        // Searchable / filterable defaults
        assert!(
            admin.contains(".searchable()"),
            "text fields should be searchable"
        );
        assert!(
            admin.contains(".filterable()"),
            "bool fields should be filterable"
        );

        // All CRUD method signatures
        assert!(admin.contains("fn list("));
        assert!(admin.contains("fn get("));
        assert!(admin.contains("fn create("));
        assert!(admin.contains("fn update("));
        assert!(admin.contains("fn delete("));

        // Model + schema imports
        assert!(admin.contains("use autumn_web::Patch;"));
        assert!(admin.contains("use crate::models::post::{Post, NewPost, UpdatePost};"));
        assert!(admin.contains("use crate::schema::posts;"));
    }

    #[test]
    fn execute_writes_admin_file_with_apply_filters_and_sort() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &["title:String".into(), "published:bool".into()],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(
            admin.contains("fn apply_filters"),
            "must have apply_filters fn"
        );
        assert!(admin.contains("fn apply_sort"), "must have apply_sort fn");
        // Search on text fields
        assert!(
            admin.contains("posts::title.ilike"),
            "title should be in ilike search"
        );
        // Bool filter
        assert!(
            admin.contains("posts::published.eq(true)"),
            "published should have bool filter"
        );
        // Sort arms for user fields
        assert!(admin.contains("\"title\""), "sort arm for title");
        assert!(admin.contains("\"published\""), "sort arm for published");
    }

    #[test]
    fn execute_writes_update_body_with_update_type() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &["title:String".into(), "published:bool".into()],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(admin.contains("let new_row: NewPost ="));
        assert!(admin.contains("let changes = UpdatePost {"));
        assert!(admin.contains("title: Patch::Set(new_row.title)"));
        assert!(admin.contains("published: Patch::Set(new_row.published)"));
        assert!(admin.contains("let diesel_changeset = changes.__to_changeset()"));
        assert!(admin.contains(".set(&diesel_changeset)"));
    }

    #[test]
    fn execute_writes_smoke_test_with_three_tests() {
        let tmp = project_with_model("post");
        let plan = plan_admin(tmp.path(), "Post", &["title:String".into()]).unwrap();
        plan.execute(Flags::default()).unwrap();

        let test = fs::read_to_string(tmp.path().join("tests/post_admin.rs")).unwrap();

        // Three test functions
        assert!(test.contains("fn post_admin_anonymous_access_is_rejected()"));
        assert!(test.contains("fn post_admin_list_loads_for_admin_user()"));
        assert!(test.contains("fn post_admin_create_redirects_for_admin_user()"));

        // Environment variable checks
        assert!(test.contains("AUTUMN_TEST_BASE_URL"));
        assert!(test.contains("AUTUMN_TEST_ADMIN_SESSION"));

        // Anonymous test expects 302/401/403
        assert!(test.contains("302") && test.contains("401") && test.contains("403"));

        // Admin list test expects 200
        assert!(test.contains("200"));

        // Route path
        assert!(test.contains("/admin/posts"));
    }

    #[test]
    fn admin_mod_rs_gets_mod_declaration() {
        let tmp = project_with_model("post");
        let plan = plan_admin(tmp.path(), "Post", &[]).unwrap();
        plan.execute(Flags::default()).unwrap();

        let mod_rs = fs::read_to_string(tmp.path().join("src/admin/mod.rs")).unwrap();
        assert!(
            mod_rs.contains("pub mod post;"),
            "src/admin/mod.rs must declare the new module; got: {mod_rs}"
        );
    }

    #[test]
    fn dry_run_does_not_write_admin_file() {
        let tmp = project_with_model("post");
        let plan = plan_admin(tmp.path(), "Post", &[]).unwrap();
        plan.execute(Flags {
            dry_run: true,
            force: false,
        })
        .unwrap();
        assert!(
            !tmp.path().join("src/admin/post.rs").exists(),
            "dry-run must not create files"
        );
    }

    #[test]
    fn collision_without_force_errors() {
        let tmp = project_with_model("post");
        // Pre-create the admin file to trigger collision detection.
        let admin_dir = tmp.path().join("src/admin");
        fs::create_dir_all(&admin_dir).unwrap();
        fs::write(admin_dir.join("post.rs"), "// existing").unwrap();

        let plan = plan_admin(tmp.path(), "Post", &[]).unwrap();
        let err = plan.execute(Flags::default()).unwrap_err();
        assert!(
            err.to_string().contains("post.rs"),
            "collision error must name the conflicting file"
        );
    }

    #[test]
    fn exclude_option_omits_field() {
        let tmp = project_with_model("post");
        let options = AdminOptions {
            exclude: vec!["body".into()],
            ..Default::default()
        };
        let plan = plan_admin_with_options(
            tmp.path(),
            "Post",
            &["title:String".into(), "body:Text".into()],
            &options,
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(
            !admin.contains("\"body\""),
            "excluded field 'body' must not appear in generated fields()"
        );
        assert!(
            admin.contains("\"title\""),
            "non-excluded 'title' must still appear"
        );
    }

    #[test]
    fn hidden_option_changes_field_kind() {
        let tmp = project_with_model("post");
        let options = AdminOptions {
            hidden: vec!["title".into()],
            ..Default::default()
        };
        let plan = plan_admin_with_options(tmp.path(), "Post", &["title:String".into()], &options)
            .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        // Should use Hidden instead of Text
        assert!(
            admin.contains("AdminField::new(\"title\", AdminFieldKind::Hidden)"),
            "hidden option must override field kind to Hidden"
        );
    }

    #[test]
    fn password_option_changes_field_kind() {
        let tmp = project_with_model("user");
        let options = AdminOptions {
            password: vec!["password_hash".into()],
            ..Default::default()
        };
        let plan = plan_admin_with_options(
            tmp.path(),
            "user",
            &["password_hash:String".into()],
            &options,
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/user.rs")).unwrap();
        assert!(
            admin.contains("AdminField::new(\"password_hash\", AdminFieldKind::Password)"),
            "password option must change kind to Password"
        );
    }

    #[test]
    fn readonly_option_adds_readonly_call() {
        let tmp = project_with_model("post");
        let options = AdminOptions {
            readonly: vec!["title".into()],
            ..Default::default()
        };
        let plan = plan_admin_with_options(tmp.path(), "Post", &["title:String".into()], &options)
            .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        // The field should appear with .readonly()
        assert!(
            admin.contains(".readonly()"),
            "readonly option must add .readonly() call"
        );
    }

    #[test]
    fn datetime_fields_are_readonly_and_optional_by_default() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &[
                "created_at:DateTime".into(),
                "updated_at:NaiveDateTime".into(),
            ],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(
            admin.contains("AdminFieldKind::DateTime"),
            "datetime fields should map to DateTime kind"
        );
        assert!(
            admin.contains(".readonly()"),
            "datetime fields must be readonly by default"
        );
        assert!(
            admin.contains(".optional()"),
            "datetime fields must be optional by default"
        );
    }

    #[test]
    fn admin_field_kind_maps_all_types() {
        use super::super::dsl::parse_field;
        let cases = [
            ("x:String", "AdminFieldKind::Text"),
            ("x:Text", "AdminFieldKind::TextArea"),
            ("x:i32", "AdminFieldKind::Integer"),
            ("x:i64", "AdminFieldKind::Integer"),
            ("x:bool", "AdminFieldKind::Boolean"),
            ("x:f32", "AdminFieldKind::Float"),
            ("x:f64", "AdminFieldKind::Float"),
            ("x:Uuid", "AdminFieldKind::Text"),
            ("x:NaiveDateTime", "AdminFieldKind::DateTime"),
            ("x:DateTime", "AdminFieldKind::DateTime"),
            ("x:Bytea", "AdminFieldKind::Hidden"),
        ];
        for (token, expected_kind) in cases {
            let field = parse_field(token).unwrap();
            assert_eq!(
                admin_field_kind(&field),
                expected_kind,
                "field token '{token}' should map to {expected_kind}"
            );
        }
    }

    #[test]
    fn datetime_fields_excluded_from_update_by_default() {
        let tmp = project_with_model("post");
        let plan = plan_admin(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "created_at:DateTime".into(),
                "updated_at:NaiveDateTime".into(),
            ],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        // created_at and updated_at are readonly by default → not in UpdatePost
        assert!(
            !admin.contains("created_at: Patch::Set(new_row.created_at)"),
            "created_at should not appear in UpdatePost (readonly by default)"
        );
        assert!(
            !admin.contains("updated_at: Patch::Set(new_row.updated_at)"),
            "updated_at should not appear in UpdatePost (readonly by default)"
        );
        // title IS writable
        assert!(
            admin.contains("title: Patch::Set(new_row.title)"),
            "title should appear in UpdatePost"
        );
    }

    #[test]
    fn select_option_generates_select_kind_with_options() {
        let tmp = project_with_model("post");
        let options = AdminOptions {
            select: vec![SelectSpec {
                field: "status".into(),
                values: vec!["draft".into(), "published".into(), "archived".into()],
            }],
            ..Default::default()
        };
        let plan = plan_admin_with_options(tmp.path(), "Post", &["status:String".into()], &options)
            .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(
            admin.contains("AdminFieldKind::Select(vec!["),
            "select option must emit Select kind"
        );
        assert!(admin.contains("\"draft\""), "option values must appear");
        assert!(admin.contains("\"published\""), "option values must appear");
        assert!(admin.contains("\"archived\""), "option values must appear");
        // Labels are humanized from values
        assert!(admin.contains("\"Draft\""), "labels must be title-cased");
    }

    #[test]
    fn select_option_bare_field_emits_empty_select() {
        let tmp = project_with_model("post");
        let options = AdminOptions {
            select: vec![SelectSpec {
                field: "status".into(),
                values: vec![],
            }],
            ..Default::default()
        };
        let plan = plan_admin_with_options(tmp.path(), "Post", &["status:String".into()], &options)
            .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/post.rs")).unwrap();
        assert!(
            admin.contains("AdminFieldKind::Select(vec![])"),
            "bare --select must emit empty Select placeholder"
        );
    }

    #[test]
    fn parse_select_specs_parses_field_with_values() {
        let specs = parse_select_specs(&["status=draft,published,archived".into()]).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].field, "status");
        assert_eq!(specs[0].values, vec!["draft", "published", "archived"]);
    }

    #[test]
    fn parse_select_specs_bare_field_produces_empty_values() {
        let specs = parse_select_specs(&["status".into()]).unwrap();
        assert_eq!(specs[0].field, "status");
        assert!(specs[0].values.is_empty());
    }

    #[test]
    fn parse_select_specs_rejects_empty_token() {
        let err = parse_select_specs(&["".into()]).unwrap_err();
        assert!(matches!(err, GenerateError::InvalidField { .. }));
    }

    #[test]
    fn no_searchable_fields_produces_valid_apply_filters() {
        // A model with only numeric/bool fields — no text-like searchable ones.
        let tmp = project_with_model("counter");
        let plan = plan_admin(
            tmp.path(),
            "counter",
            &["count:i64".into(), "active:bool".into()],
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let admin = fs::read_to_string(tmp.path().join("src/admin/counter.rs")).unwrap();
        // apply_filters must still be present and compile-valid (no ilike calls)
        assert!(
            admin.contains("fn apply_filters"),
            "apply_filters must exist"
        );
        assert!(
            !admin.contains(".ilike("),
            "no ilike when there are no searchable fields"
        );
    }
}
