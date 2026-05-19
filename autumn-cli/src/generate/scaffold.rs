//! `autumn generate scaffold` — full CRUD scaffold.
//!
//! Builds on top of [`model::plan_model`](super::model::plan_model) and adds:
//!
//! - A `#[repository(Model, api = "/api/<plural>")]` block for JSON reads/writes.
//! - HTML route handlers for `index`, `show`, `new_form`, `create`, `edit_form`,
//!   and `update`, returning Maud `Markup`.
//! - A `tests/<snake>.rs` smoke test that asserts the index route returns 200.
//! - Updates to `src/main.rs` registering all new routes in `routes![ … ]`.

use std::path::Path;

use super::dsl::{Field, FieldKind, parse_fields};
use super::emit::Plan;
use super::model::{
    ModelOptions, field_by_name, parse_model_metadata, plan_cargo_deps, plan_model_with_options,
};
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::{add_mod_declaration, update_main_rs};
use super::{Flags, GenerateError, ensure_project_root, timestamp_now};

/// Extra dependencies the *scaffold* generator's output requires on top of
/// [`super::model::MODEL_DEPS`] — `maud` for HTML rendering and URL-encoded
/// form helpers for blank nullable-field normalization.
const SCAFFOLD_EXTRA_DEPS: &[(&str, &str)] = &[
    ("maud", "{ version = \"0.27\", features = [\"axum\"] }"),
    ("serde_urlencoded", "\"0.7\""),
    ("url", "\"2\""),
];

/// Optional metadata applied by `autumn generate scaffold`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScaffoldOptions {
    /// Model-level field metadata.
    pub model: ModelOptions,
    /// Repository derived-query specs in `method:field` form.
    pub queries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuerySpec {
    method: String,
    field_name: String,
    rust_type: String,
}

/// Compute the file actions for `autumn generate scaffold`.
///
/// # Errors
/// Surfaces any planning error from the underlying [`plan_model`] call as
/// well as project-layout problems (missing `src/main.rs`).
#[cfg(test)]
pub fn plan_scaffold(
    project_root: &Path,
    name: &str,
    field_tokens: &[String],
    timestamp: &str,
) -> Result<Plan, GenerateError> {
    plan_scaffold_with_options(
        project_root,
        name,
        field_tokens,
        timestamp,
        &ScaffoldOptions::default(),
    )
}

/// Compute the file actions for `autumn generate scaffold`, using optional
/// metadata flags.
///
/// # Errors
/// Surfaces any planning error from the underlying model generation as well
/// as project-layout, repository query, and metadata problems.
pub fn plan_scaffold_with_options(
    project_root: &Path,
    name: &str,
    field_tokens: &[String],
    timestamp: &str,
    options: &ScaffoldOptions,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;
    let mut plan =
        plan_model_with_options(project_root, name, field_tokens, timestamp, &options.model)?;
    let fields = parse_fields(field_tokens)?;
    let metadata = parse_model_metadata(&fields, &options.model)?;
    let queries = parse_query_specs(&fields, &options.queries)?;
    let form_fields = fields
        .iter()
        .filter(|field| !metadata.defaults().contains_key(&field.name))
        .cloned()
        .collect::<Vec<_>>();
    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let plural = pluralize(&snake_name);

    // Repository file under `src/repositories/<snake>.rs`
    let repos_dir = project_root.join("src").join("repositories");
    plan.create(
        repos_dir.join(format!("{snake_name}.rs")),
        render_repository_file(
            &pascal_name,
            &snake_name,
            &queries,
            options.model.soft_delete,
        ),
    );
    let repo_mod_path = repos_dir.join("mod.rs");
    plan.modify(
        repo_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&repo_mod_path), &snake_name),
    );

    // Route file under `src/routes/<plural>.rs`
    let routes_dir = project_root.join("src").join("routes");
    plan.create(
        routes_dir.join(format!("{plural}.rs")),
        render_routes_file(&pascal_name, &snake_name, &plural, &form_fields),
    );
    let route_mod_path = routes_dir.join("mod.rs");
    plan.modify(
        route_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&route_mod_path), &plural),
    );

    // Smoke test under `tests/<snake>.rs`
    plan.create(
        project_root.join("tests").join(format!("{snake_name}.rs")),
        render_smoke_test(&pascal_name, &plural),
    );

    // `src/main.rs` updates: declare modules + register all new routes.
    let main_path = project_root.join("src").join("main.rs");
    let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
        GenerateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing {}", main_path.display()),
        ))
    })?;
    let route_entries = main_route_entries(&plural, &snake_name);
    let updated = update_main_rs(
        &main_existing,
        &["models", "routes", "schema", "repositories"],
        &route_entries,
    );
    plan.modify(main_path, updated);

    // The Maud `html!` macro pulls in a direct `maud` dep on top of the
    // model's deps. Both modify actions target Cargo.toml, so we combine
    // them into a single deduplicated call — otherwise the second write
    // would clobber the first (each rendering is computed at plan time
    // against the on-disk Cargo.toml).
    plan.actions.retain(|a| !a.path().ends_with("Cargo.toml"));
    let mut combined: Vec<(&str, &str)> = super::model::MODEL_DEPS
        .iter()
        .copied()
        .chain(SCAFFOLD_EXTRA_DEPS.iter().copied())
        .collect();
    if metadata.has_validator_rules() {
        combined.push((
            "validator",
            "{ version = \"0.20\", features = [\"derive\"] }",
        ));
    }
    plan_cargo_deps(&mut plan, project_root, &combined);

    Ok(plan)
}

/// CLI entry point.
pub fn run(name: &str, field_tokens: &[String], flags: Flags, options: &ScaffoldOptions) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let timestamp = timestamp_now();
    let plan = plan_scaffold_with_options(&cwd, name, field_tokens, &timestamp, options);
    match plan.and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn read_or_empty(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn parse_query_specs(
    fields: &[Field],
    queries: &[String],
) -> Result<Vec<QuerySpec>, GenerateError> {
    let mut parsed = Vec::with_capacity(queries.len());
    for query in queries {
        let (method, field_name) =
            query
                .split_once(':')
                .ok_or_else(|| GenerateError::InvalidField {
                    token: query.clone(),
                    reason: "expected `method:field`, for example `find_by_tag:tag`".into(),
                })?;
        let method = method.trim();
        let field_name = field_name.trim();
        if !method.starts_with("find_by_") || !is_valid_fn_name(method) {
            return Err(GenerateError::InvalidField {
                token: query.clone(),
                reason: "query method must be a valid `find_by_<field>` function name".into(),
            });
        }
        let method_field = method
            .strip_prefix("find_by_")
            .expect("prefix checked above");
        let field =
            field_by_name(fields, field_name).ok_or_else(|| GenerateError::InvalidField {
                token: query.clone(),
                reason: format!("unknown field '{field_name}'"),
            })?;
        if method_field != field_name {
            return Err(GenerateError::InvalidField {
                token: query.clone(),
                reason: format!(
                    "query method suffix '{method_field}' must match field '{field_name}'"
                ),
            });
        }
        if parsed.iter().any(|spec: &QuerySpec| spec.method == method) {
            return Err(GenerateError::InvalidField {
                token: query.clone(),
                reason: format!("duplicate query method '{method}'"),
            });
        }
        parsed.push(QuerySpec {
            method: method.to_owned(),
            field_name: field_name.to_owned(),
            rust_type: field.rust_type(),
        });
    }
    Ok(parsed)
}

fn is_valid_fn_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn render_repository_file(
    pascal_name: &str,
    snake_name: &str,
    queries: &[QuerySpec],
    soft_delete: bool,
) -> String {
    let plural = pluralize(snake_name);
    let query_body = render_repository_queries(pascal_name, queries);
    let soft_delete_attr = if soft_delete { ", soft_delete" } else { "" };
    format!(
        "//! Generated by `autumn generate scaffold`.\n\
         //!\n\
         //! `#[repository]` auto-generates CRUD methods and JSON REST handlers.\n\
         //! The scaffold registers only read handlers in `src/main.rs` by\n\
         //! default. Mount mutating API handlers only after adding a policy.\n\
         \n\
         use crate::models::{snake_name}::{{{pascal_name}, New{pascal_name}, Update{pascal_name}}};\n\
         use crate::schema::{plural};\n\
         \n\
         #[autumn_web::repository({pascal_name}, api = \"/api/{plural}\"{soft_delete_attr})]\n\
         pub trait {pascal_name}Repository {{\n\
{query_body}\
         }}\n"
    )
}

fn render_repository_queries(pascal_name: &str, queries: &[QuerySpec]) -> String {
    let mut out = String::with_capacity(queries.len() * 64);
    for query in queries {
        use std::fmt::Write as _;
        let _ = writeln!(
            out,
            "    fn {method}({field}: {rust_type}) -> Vec<{pascal_name}>;",
            method = query.method,
            field = query.field_name,
            rust_type = query.rust_type,
        );
    }
    out
}

#[allow(
    clippy::too_many_lines,
    reason = "This is a single template — splitting it produces less readable output, \
              not more. The whole point is one place that prints one file."
)]
fn render_routes_file(
    pascal_name: &str,
    snake_name: &str,
    plural: &str,
    fields: &[Field],
) -> String {
    let create_inputs = render_create_form_inputs(fields);
    let edit_inputs = render_edit_form_inputs(fields);
    let update_columns = render_update_columns(plural, fields);
    let nullable_field_match = render_nullable_field_match(fields);
    let has_attachments = has_attachment_fields(fields);
    // Keep forms URL-encoded; attachment uploads use a separate direct-upload
    // flow that bypasses form submission.
    let form_enctype = "";
    format!(
        r#"//! Generated by `autumn generate scaffold`.
//!
//! HTML route handlers for the resource. Edit freely — once generated,
//! these are ordinary user code.
{attachment_note}
use autumn_web::extract::Path;
use autumn_web::pagination::{{Page, PageRequest}};
use autumn_web::reexports::axum::body::Bytes;
use autumn_web::security::{{CsrfFormField, CsrfToken}};
use autumn_web::{{AutumnError, AutumnResult, Db, Markup, get, html, post, secured}};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{snake_name}::{{{pascal_name}, New{pascal_name}}};
use crate::repositories::{snake_name}::{{{pascal_name}Repository, Pg{pascal_name}Repository}};
use crate::schema::{plural};"#,
        attachment_note = if has_attachments {
            "//!\n\
             //! This scaffold includes file-attachment fields. To enable uploads:\n\
             //!\n\
             //! 1. Add `autumn-web = {{ features = [\"storage\", \"multipart\"] }}` to Cargo.toml.\n\
             //! 2. Configure `[storage]` in `autumn.toml` (local disk for dev, S3 for prod).\n\
             //! 3. In the `create` and `update` handlers, replace `Bytes` with\n\
             //!    `autumn_web::extract::Multipart` and call\n\
             //!    `field.save_to_blob_store(&*store, key).await?` for each attachment field.\n\
             //! See `docs/guide/storage.md` for the full worked example.\n\
             //!\n\
             //! For direct browser-to-storage uploads (bypassing the app process),\n\
             //! see `docs/guide/storage.md#direct-uploads` and use\n\
             //! `autumn_web::storage::BlobStore::presign_put` in a CSRF-protected endpoint."
        } else {
            ""
        },
    ) + &format!(
        r#"

fn csrf_input(csrf: Option<&CsrfToken>, field: Option<&CsrfFormField>) -> Markup {{
    let csrf_field_name = field.map(|field| field.0.as_str()).unwrap_or("_csrf");
    html! {{
        @if let Some(csrf) = csrf {{
            input type="hidden" name=(csrf_field_name) value=(csrf.token());
        }}
    }}
}}

/// Wrap content in a minimal HTML layout. Replace with your real layout
/// once you wire in Tailwind / your design system.
fn layout(title: &str, content: Markup) -> Markup {{
    html! {{
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {{
            head {{
                meta charset="utf-8";
                title {{ (title) }}
            }}
            body {{ (content) }}
        }}
    }}
}}

/// Render Previous / page-indicator / Next navigation for a paginated list.
///
/// Links are htmx-friendly: the `hx-get` attribute targets the whole page body
/// so progressive-enhancement apps get smooth partial updates without extra JS.
fn pagination_nav<T>(page: &Page<T>, base_url: &str) -> Markup {{
    html! {{
        nav aria-label="Pagination" {{
            @if page.has_previous {{
                a href=(format!("{{}}?page={{}}&size={{}}", base_url, page.page - 1, page.size))
                   hx-get=(format!("{{}}?page={{}}&size={{}}", base_url, page.page - 1, page.size))
                   hx-target="body" {{
                    "← Previous"
                }}
                " "
            }}
            span {{
                "Page " (page.page) " of " (page.total_pages)
                " (" (page.total_elements) " total)"
            }}
            @if page.has_next {{
                " "
                a href=(format!("{{}}?page={{}}&size={{}}", base_url, page.page + 1, page.size))
                   hx-get=(format!("{{}}?page={{}}&size={{}}", base_url, page.page + 1, page.size))
                   hx-target="body" {{
                    "Next →"
                }}
            }}
        }}
    }}
}}

/// `GET /{plural}` — paginated list of {snake_name}s.
///
/// Accepts `?page=N&size=M` query parameters via the [`PageRequest`] extractor.
/// Out-of-range or missing values are clamped silently — list endpoints never
/// return HTTP 400 for bad paging parameters.
#[get("/{plural}")]
pub async fn index(
    page_req: PageRequest,
    repo: Pg{pascal_name}Repository,
) -> AutumnResult<Markup> {{
    let page_data: Page<{pascal_name}> = repo.page(&page_req).await?;
    Ok(layout("{pascal_name} index", html! {{
        h1 {{ "{pascal_name}s" }}
        a href="/{plural}/new" {{ "New {pascal_name}" }}
        ul {{
            @for row in &page_data.content {{
                li {{ a href=(format!("/{plural}/{{}}", row.id)) {{ (row.id) }} }}
            }}
        }}
        (pagination_nav(&page_data, "/{plural}"))
    }}))
}}

/// `GET /{plural}/{{id}}` — show one {snake_name}.
#[get("/{plural}/{{id}}")]
pub async fn show(id: Path<i64>, mut db: Db) -> AutumnResult<Markup> {{
    let row: {pascal_name} = {plural}::table
        .find(*id)
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;
    Ok(layout(&format!("{pascal_name} #{{}}", row.id), html! {{
        h1 {{ "{pascal_name} #" (row.id) }}
        a href="/{plural}" {{ "Back to list" }}
    }}))
}}

/// `GET /{plural}/new` — render the new-{snake_name} form.
#[secured]
#[get("/{plural}/new")]
pub async fn new_form(
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Markup> {{
    Ok(layout("New {pascal_name}", html! {{
        h1 {{ "New {pascal_name}" }}
        form action="/{plural}" method="post"{form_enctype} {{
            (csrf_input(csrf.as_ref(), csrf_field.as_ref()))
{create_inputs}            button type="submit" {{ "Create" }}
        }}
    }}))
}}

/// `POST /{plural}` — accept a form submission and create a {snake_name}.
#[secured]
#[post("/{plural}")]
pub async fn create(mut db: Db, body: Bytes) -> AutumnResult<Markup> {{
    let new = decode_form(body)?;
    diesel::insert_into({plural}::table)
        .values(&new)
        .execute(&mut *db)
        .await?;
    Ok(redirect_to("/{plural}"))
}}

/// `GET /{plural}/{{id}}/edit` — render the edit form. Submission goes to
/// the `update` handler below as a plain HTML POST (browsers can't submit
/// PUT directly without JS); the auto-generated JSON `PUT /api/{plural}/{{id}}`
/// remains available for API clients.
#[secured]
#[get("/{plural}/{{id}}/edit")]
pub async fn edit_form(
    id: Path<i64>,
    mut db: Db,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Markup> {{
    let row: {pascal_name} = {plural}::table
        .find(*id)
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;
    Ok(layout(&format!("Edit {pascal_name} #{{}}", row.id), html! {{
        h1 {{ "Edit {pascal_name} #" (row.id) }}
        form action=(format!("/{plural}/{{}}/update", row.id)) method="post"{form_enctype} {{
            (csrf_input(csrf.as_ref(), csrf_field.as_ref()))
{edit_inputs}            button type="submit" {{ "Save" }}
        }}
    }}))
}}

/// `POST /{plural}/{{id}}/update` — apply form data to a row, then redirect
/// to its show page. Uses column-by-column `diesel::update().set(...)` (same
/// convention as `examples/todo-app`) so we don't need `AsChangeset` on the
/// `New{pascal_name}` insert type.
#[secured]
#[post("/{plural}/{{id}}/update")]
pub async fn update(
    id: Path<i64>,
    mut db: Db,
    body: Bytes,
) -> AutumnResult<Markup> {{
    let form = decode_form(body)?;
    let updated = diesel::update({plural}::table.find(*id))
        .set(({update_columns}))
        .execute(&mut *db)
        .await?;
    if updated == 0 {{
        return Err(AutumnError::not_found_msg(format!(
            "{pascal_name} with id {{}} not found", *id
        )));
    }}
    Ok(redirect_to(&format!("/{plural}/{{}}", *id)))
}}

fn decode_form(body: Bytes) -> AutumnResult<New{pascal_name}> {{
    let pairs: Vec<_> = url::form_urlencoded::parse(body.as_ref())
        .filter(|(key, value)| !(value.is_empty() && is_nullable_form_field(key)))
        .collect();
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(key, value)| (key.as_ref(), value.as_ref())))
        .finish();

    serde_urlencoded::from_str(&encoded)
        .map_err(|err| AutumnError::bad_request_msg(format!("invalid form submission: {{err}}")))
}}

fn is_nullable_form_field(name: &str) -> bool {{
    {nullable_field_match}
}}

fn redirect_to(url: &str) -> Markup {{
    html! {{
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html {{ head {{
            meta http-equiv="refresh" content=(format!("0;url={{url}}"));
        }} body {{ p {{ "Redirecting to " a href=(url) {{ (url) }} "…" }} }} }}
    }}
}}
"#
    )
}

/// Whether any field in `fields` is a file attachment.
pub(crate) fn has_attachment_fields(fields: &[Field]) -> bool {
    fields.iter().any(|f| f.kind.is_attachment())
}

fn render_create_form_inputs(fields: &[Field]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for f in fields {
        if f.kind.is_attachment() {
            // Attachment fields render as file inputs; the form must use
            // enctype="multipart/form-data" (set by render_routes_file when
            // attachment fields are present). Upload logic (storage backend
            // + blob binding) requires the `autumn-web` `storage` and
            // `multipart` features and is left for the app author to wire.
            let _ = writeln!(
                out,
                "            label {{ \"{name}\" }} input type=\"file\" name=\"{name}\";",
                name = f.name
            );
        } else {
            let required = required_attr(f);
            let _ = writeln!(
                out,
                "            label {{ \"{name}\" }} input type=\"text\" name=\"{name}\"{required};",
                name = f.name,
                required = required
            );
        }
    }
    out
}

fn render_edit_form_inputs(fields: &[Field]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for f in fields {
        if f.kind.is_attachment() {
            let _ = writeln!(
                out,
                "            label {{ \"{name}\" }} input type=\"file\" name=\"{name}\";",
                name = f.name
            );
        } else {
            let value = edit_value_expr(f);
            let required = required_attr(f);
            let _ = writeln!(
                out,
                "            label {{ \"{name}\" }} input type=\"text\" name=\"{name}\" value=({value}){required};",
                name = f.name,
                value = value,
                required = required
            );
        }
    }
    out
}

const fn required_attr(field: &Field) -> &'static str {
    if field.nullable { "" } else { " required" }
}

fn edit_value_expr(field: &Field) -> String {
    let name = &field.name;
    match (field.nullable, field.kind) {
        // Attachment fields don't render a value in text inputs — they have
        // their own <input type="file"> generated by render_edit_form_inputs.
        (_, FieldKind::Attachment) => String::new(),
        (true, FieldKind::Bytea) => {
            format!(
                "row.{name}.as_ref().map(|value| String::from_utf8_lossy(value).to_string()).unwrap_or_default()"
            )
        }
        (true, _) => {
            format!("row.{name}.as_ref().map(ToString::to_string).unwrap_or_default()")
        }
        (false, FieldKind::Bytea) => {
            format!("String::from_utf8_lossy(&row.{name}).to_string()")
        }
        (false, _) => format!("row.{name}.to_string()"),
    }
}

fn render_nullable_field_match(fields: &[Field]) -> String {
    let names = fields
        .iter()
        .filter(|field| field.nullable)
        .map(|field| format!("\"{}\"", field.name))
        .collect::<Vec<_>>();
    if names.is_empty() {
        "false".to_owned()
    } else {
        format!("matches!(name, {})", names.join(" | "))
    }
}

/// Render the column-update tuple body for the `update` handler. Emits
/// `tablename::field.eq(form.field.clone()), …` per user field, leaving the
/// auto-managed `id` and `created_at` columns alone. With no user fields the
/// body is empty (Diesel accepts `set(())` as a no-op update).
///
/// ⚡ Bolt optimization: Avoids intermediate `Vec` allocations during string formatting
/// by pre-allocating capacity and utilizing `std::fmt::Write` sequentially.
fn render_update_columns(plural: &str, fields: &[Field]) -> String {
    use std::fmt::Write;
    // Estimate 50 chars per field
    let mut out = String::with_capacity(fields.len() * 50);
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(
            out,
            "{plural}::{name}.eq(form.{name}.clone())",
            name = f.name
        )
        .unwrap();
    }
    out
}

fn render_smoke_test(pascal_name: &str, plural: &str) -> String {
    format!(
        "//! Smoke test generated by `autumn generate scaffold`.\n\
         //!\n\
         //! Compiles against the project's stock dependency set (just\n\
         //! `autumn-web`) so it lights up in CI immediately. Hit the route\n\
         //! on a real server with the steps documented in the test body.\n\
         \n\
         #[test]\n\
         fn {plural}_index_returns_200_when_server_is_running() {{\n\
             let Ok(base) = std::env::var(\"AUTUMN_TEST_BASE_URL\") else {{\n\
                 eprintln!(\"skipping: AUTUMN_TEST_BASE_URL not set\");\n\
                 return;\n\
             }};\n\
             // Hit the running app at $AUTUMN_TEST_BASE_URL -- we go via raw\n\
             // `std::net::TcpStream` to avoid forcing reqwest into the\n\
             // project's dependency graph. Once the user wires in their\n\
             // preferred HTTP client they should replace this body.\n\
             let base = base.trim_end_matches('/');\n\
             let url = format!(\"{{base}}/{plural}\");\n\
             let host_port = base\n\
                 .trim_start_matches(\"http://\")\n\
                 .trim_start_matches(\"https://\");\n\
             let mut stream = std::net::TcpStream::connect(host_port)\n\
                 .unwrap_or_else(|_| panic!(\"could not connect to {{url}}\"));\n\
             use std::io::{{Read, Write}};\n\
             let req = format!(\n\
                 \"GET /{plural} HTTP/1.1\\r\\nHost: {{host_port}}\\r\\nConnection: close\\r\\n\\r\\n\"\n\
             );\n\
             stream.write_all(req.as_bytes()).expect(\"write failed\");\n\
             let mut response = String::new();\n\
             stream.read_to_string(&mut response).expect(\"read failed\");\n\
             assert!(\n\
                 response.starts_with(\"HTTP/1.1 200\")\n\
                     || response.starts_with(\"HTTP/1.0 200\"),\n\
                 \"{pascal_name} index did not return 200:\\n{{response}}\"\n\
             );\n\
             assert!(\n\
                 response.contains(\"{pascal_name}s\"),\n\
                 \"{pascal_name} index did not render the generated template:\\n{{response}}\"\n\
             );\n\
         }}\n",
    )
}

fn main_route_entries(plural: &str, snake_name: &str) -> Vec<String> {
    vec![
        format!("routes::{plural}::index"),
        format!("routes::{plural}::show"),
        format!("routes::{plural}::new_form"),
        format!("routes::{plural}::create"),
        format!("routes::{plural}::edit_form"),
        format!("routes::{plural}::update"),
        format!("repositories::{snake_name}::{snake_name}_api_list"),
        format!("repositories::{snake_name}::{snake_name}_api_get"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project_with_main(template: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), template).unwrap();
        tmp
    }

    fn default_main() -> &'static str {
        r#"use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str { "ok" }

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .run()
        .await;
}
"#
    }

    #[test]
    fn plan_creates_full_scaffold() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "body:Text".into(),
                "published:bool".into(),
            ],
            "20260427000000",
        )
        .unwrap();
        let paths: Vec<String> = plan
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(&plan.project_root)
                    .unwrap()
                    .display()
                    .to_string()
                    // Normalize for cross-platform comparisons (Windows uses `\`).
                    .replace('\\', "/")
            })
            .collect();
        for expected in [
            "src/models/post.rs",
            "src/models/mod.rs",
            "migrations/20260427000000_create_posts/up.sql",
            "migrations/20260427000000_create_posts/down.sql",
            "src/schema.rs",
            "src/repositories/post.rs",
            "src/repositories/mod.rs",
            "src/routes/posts.rs",
            "src/routes/mod.rs",
            "tests/post.rs",
            "src/main.rs",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "missing expected action for {expected}; got {paths:?}"
            );
        }
    }

    #[test]
    fn plan_errors_when_main_rs_missing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        let err = plan_scaffold(tmp.path(), "Post", &[], "20260427000000").unwrap_err();
        assert!(matches!(err, GenerateError::Io(_)));
    }

    #[test]
    fn execute_writes_a_routes_file_referencing_model() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/posts.rs")).unwrap();
        assert!(routes.contains("use crate::models::post::{Post, NewPost};"));
        assert!(routes.contains("#[get(\"/posts\")]"));
        assert!(routes.contains("#[get(\"/posts/{id}\")]"));
        assert!(
            !routes.contains("#[secured]\n#[get(\"/posts\")]"),
            "index should be reachable by the five-command scaffold smoke test"
        );
        assert!(
            !routes.contains("#[secured]\n#[get(\"/posts/{id}\")]"),
            "read-only show pages should stay public when generated"
        );
        assert!(routes.contains("#[get(\"/posts/new\")]"));
        assert!(routes.contains("#[post(\"/posts\")]"));
        assert!(routes.contains("#[get(\"/posts/{id}/edit\")]"));
        // The HTML edit form posts to a regular `POST /posts/{id}/update`
        // (browsers can't submit PUT natively); the JSON `PUT /api/posts/{id}`
        // remains available via the auto-generated repository handler.
        assert!(routes.contains("#[post(\"/posts/{id}/update\")]"));
        assert!(routes.contains("pub async fn new_form("));
        assert!(routes.contains("Ok(layout(\"New Post\""));
        assert!(routes.contains("posts::title.eq(form.title.clone())"));
        // `execute()` returns the affected row count — `Ok(0)` means the id
        // didn't exist, and we must return 404 instead of redirecting as if
        // the save succeeded. DB errors stay distinct from "not found".
        assert!(routes.contains("if updated == 0"));
        assert!(routes.contains("AutumnError::not_found_msg"));
        // The HTML edit form must point at the new HTML update handler, not
        // the JSON PUT endpoint — browsers cannot submit PUT without JS.
        assert!(routes.contains("/posts/{}/update"));
        assert!(!routes.contains("/api/posts/{}\""));
        // Update and delete remain available through JSON REST.
        assert!(!routes.contains("#[put("));
        assert!(!routes.contains("#[delete("));
    }

    #[test]
    fn execute_writes_csrf_aware_form_handlers() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/posts.rs")).unwrap();
        assert!(routes.contains("use autumn_web::security::{CsrfFormField, CsrfToken};"));
        assert!(routes.contains("fn csrf_input("));
        assert!(routes.contains("input type=\"hidden\" name=(csrf_field_name"));
        assert!(routes.contains("value=(csrf.token());"));
        assert!(routes.contains("pub async fn new_form("));
        assert!(routes.contains("csrf: Option<CsrfToken>"));
        assert!(routes.contains("csrf_field: Option<CsrfFormField>"));
        assert!(routes.contains("(csrf_input(csrf.as_ref(), csrf_field.as_ref()))"));
        assert!(routes.contains("pub async fn edit_form("));
    }

    #[test]
    fn execute_writes_edit_form_with_prefilled_values_and_nullable_optional_inputs() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "subtitle:Option<String>".into(),
                "views:Option<i64>".into(),
            ],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/posts.rs")).unwrap();
        assert!(
            routes.contains(
                r#"label { "title" } input type="text" name="title" value=(row.title.to_string()) required;"#
            ),
            "edit form must prefill required fields from the loaded row: {routes}"
        );
        assert!(
            routes.contains(
                r#"label { "subtitle" } input type="text" name="subtitle" value=(row.subtitle.as_ref().map(ToString::to_string).unwrap_or_default());"#
            ),
            "edit form must prefill nullable text fields from the loaded row: {routes}"
        );
        assert!(
            routes.contains(
                r#"label { "views" } input type="text" name="views" value=(row.views.as_ref().map(ToString::to_string).unwrap_or_default());"#
            ),
            "edit form must prefill nullable numeric fields from the loaded row: {routes}"
        );
        assert!(
            routes.contains(r#"label { "subtitle" } input type="text" name="subtitle";"#),
            "new form must not mark nullable fields required: {routes}"
        );
        assert!(
            routes.contains(r#"label { "views" } input type="text" name="views";"#),
            "new form must not mark nullable numeric fields required: {routes}"
        );
    }

    #[test]
    fn execute_writes_form_decoder_that_drops_blank_nullable_fields() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &[
                "title:String".into(),
                "nickname:Option<String>".into(),
                "views:Option<i64>".into(),
                "published_at:Option<NaiveDateTime>".into(),
                "token:Option<Uuid>".into(),
            ],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/posts.rs")).unwrap();
        assert!(
            routes.contains("use autumn_web::reexports::axum::body::Bytes;"),
            "generated routes must be able to inspect raw form bytes: {routes}"
        );
        assert!(
            routes.contains("pub async fn create(mut db: Db, body: Bytes)"),
            "create must decode after blank nullable normalization: {routes}"
        );
        assert!(
            routes.contains(
                "pub async fn update(\n    id: Path<i64>,\n    mut db: Db,\n    body: Bytes,\n)"
            ),
            "update must decode after blank nullable normalization: {routes}"
        );
        assert!(
            routes.contains("let new = decode_form(body)?;"),
            "create handler must use the generated decoder: {routes}"
        );
        assert!(
            routes.contains("let form = decode_form(body)?;"),
            "update handler must use the generated decoder: {routes}"
        );
        assert!(
            routes.contains(r#"matches!(name, "nickname" | "views" | "published_at" | "token")"#),
            "decoder must drop blank submissions for every nullable field: {routes}"
        );
    }

    #[test]
    fn execute_writes_a_repository_with_json_api_attribute() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(tmp.path(), "Post", &[], "20260427000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let repo = fs::read_to_string(tmp.path().join("src/repositories/post.rs")).unwrap();
        assert!(repo.contains("#[autumn_web::repository(Post, api = \"/api/posts\")]"));
        assert!(repo.contains("pub trait PostRepository"));
    }

    #[test]
    fn execute_updates_main_rs_with_mods_and_routes() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(main.contains("mod models;"));
        assert!(main.contains("mod routes;"));
        assert!(main.contains("mod schema;"));
        assert!(main.contains("mod repositories;"));
        assert!(main.contains("routes::posts::index"));
        assert!(main.contains("routes::posts::show"));
        assert!(main.contains("routes::posts::new_form"));
        assert!(main.contains("routes::posts::create"));
        assert!(main.contains("routes::posts::edit_form"));
        assert!(main.contains("routes::posts::update"));
        assert!(main.contains("repositories::post::post_api_list"));
        assert!(main.contains("repositories::post::post_api_get"));
        assert!(!main.contains("repositories::post::post_api_create"));
        assert!(!main.contains("repositories::post::post_api_update"));
        assert!(!main.contains("repositories::post::post_api_delete"));
    }

    #[test]
    fn execute_writes_smoke_test() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(tmp.path(), "Post", &[], "20260427000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let test = fs::read_to_string(tmp.path().join("tests/post.rs")).unwrap();
        assert!(test.contains("posts_index_returns_200_when_server_is_running"));
        assert!(test.contains("AUTUMN_TEST_BASE_URL"));
        assert!(!test.contains("AUTUMN_TEST_SESSION_COOKIE"));
        assert!(!test.contains("Cookie: {session_cookie}"));
        assert!(test.contains("/posts"));
    }

    #[test]
    fn dry_run_does_not_modify_main() {
        let tmp = project_with_main(default_main());
        let original = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        let plan = plan_scaffold(tmp.path(), "Post", &[], "20260427000000").unwrap();
        plan.execute(Flags {
            dry_run: true,
            force: false,
        })
        .unwrap();
        let after = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn collision_lists_existing_files_without_force() {
        let tmp = project_with_main(default_main());
        // Pre-create one of the files so the next run collides.
        let dir = tmp.path().join("src/models");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("post.rs"), "// existing").unwrap();
        let plan = plan_scaffold(tmp.path(), "Post", &[], "20260427000000").unwrap();
        let err = plan.execute(Flags::default()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("post.rs"));
    }

    // ── Soft-delete scaffold generation (issue #689) ──────────────

    #[test]
    fn scaffold_soft_delete_repository_annotation_includes_soft_delete() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold_with_options(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
            &ScaffoldOptions {
                model: ModelOptions {
                    soft_delete: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let repo = fs::read_to_string(tmp.path().join("src/repositories/post.rs")).unwrap();
        assert!(
            repo.contains("soft_delete"),
            "repository file must include soft_delete in the #[repository] annotation: {repo}"
        );
    }

    #[test]
    fn scaffold_soft_delete_model_includes_deleted_at_field() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold_with_options(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
            &ScaffoldOptions {
                model: ModelOptions {
                    soft_delete: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let model = fs::read_to_string(tmp.path().join("src/models/post.rs")).unwrap();
        assert!(
            model.contains("deleted_at"),
            "model struct must include deleted_at field when soft_delete is enabled: {model}"
        );
    }

    #[test]
    fn scaffold_without_soft_delete_does_not_include_soft_delete_annotation() {
        let tmp = project_with_main(default_main());
        let plan = plan_scaffold(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let repo = fs::read_to_string(tmp.path().join("src/repositories/post.rs")).unwrap();
        assert!(
            !repo.contains("soft_delete"),
            "repository without soft_delete must not include soft_delete annotation: {repo}"
        );
    }
}
