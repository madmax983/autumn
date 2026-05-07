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

use super::dsl::{Field, parse_fields};
use super::emit::Plan;
use super::model::{
    ModelOptions, field_by_name, parse_model_metadata, plan_cargo_deps, plan_model_with_options,
};
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::{add_mod_declaration, update_main_rs};
use super::{Flags, GenerateError, ensure_project_root, timestamp_now};

/// Extra dependencies the *scaffold* generator's output requires on top of
/// [`super::model::MODEL_DEPS`] — `maud` for HTML rendering in routes.
const SCAFFOLD_EXTRA_DEPS: &[(&str, &str)] =
    &[("maud", "{ version = \"0.27\", features = [\"axum\"] }")];

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
    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let plural = pluralize(&snake_name);

    // Repository file under `src/repositories/<snake>.rs`
    let repos_dir = project_root.join("src").join("repositories");
    plan.create(
        repos_dir.join(format!("{snake_name}.rs")),
        render_repository_file(&pascal_name, &snake_name, &queries),
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
        render_routes_file(&pascal_name, &snake_name, &plural, &fields),
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
    let plan = if *options == ScaffoldOptions::default() {
        plan_scaffold(&cwd, name, field_tokens, &timestamp)
    } else {
        plan_scaffold_with_options(&cwd, name, field_tokens, &timestamp, options)
    };
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
        let field =
            field_by_name(fields, field_name).ok_or_else(|| GenerateError::InvalidField {
                token: query.clone(),
                reason: format!("unknown field '{field_name}'"),
            })?;
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

fn render_repository_file(pascal_name: &str, snake_name: &str, queries: &[QuerySpec]) -> String {
    let plural = pluralize(snake_name);
    let query_body = render_repository_queries(pascal_name, queries);
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
         #[autumn_web::repository({pascal_name}, api = \"/api/{plural}\")]\n\
         pub trait {pascal_name}Repository {{\n\
{query_body}\
         }}\n"
    )
}

fn render_repository_queries(pascal_name: &str, queries: &[QuerySpec]) -> String {
    let mut out = String::new();
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
    let inputs = render_form_inputs(fields);
    let update_columns = render_update_columns(plural, fields);
    format!(
        r#"//! Generated by `autumn generate scaffold`.
//!
//! HTML route handlers for the resource. Edit freely — once generated,
//! these are ordinary user code.

use autumn_web::extract::{{Form, Path}};
use autumn_web::{{AutumnError, AutumnResult, Db, Markup, get, html, post, secured}};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{snake_name}::{{{pascal_name}, New{pascal_name}}};
use crate::schema::{plural};

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

/// `GET /{plural}` — list every {snake_name}.
#[get("/{plural}")]
pub async fn index(mut db: Db) -> AutumnResult<Markup> {{
    let rows: Vec<{pascal_name}> = {plural}::table
        .select({pascal_name}::as_select())
        .load(&mut *db)
        .await?;
    Ok(layout("{pascal_name} index", html! {{
        h1 {{ "{pascal_name}s" }}
        a href="/{plural}/new" {{ "New {pascal_name}" }}
        ul {{
            @for row in &rows {{
                li {{ a href=(format!("/{plural}/{{}}", row.id)) {{ (row.id) }} }}
            }}
        }}
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
pub async fn new_form() -> AutumnResult<Markup> {{
    Ok(layout("New {pascal_name}", html! {{
        h1 {{ "New {pascal_name}" }}
        form action="/{plural}" method="post" {{
{inputs}            button type="submit" {{ "Create" }}
        }}
    }}))
}}

/// `POST /{plural}` — accept a form submission and create a {snake_name}.
#[secured]
#[post("/{plural}")]
pub async fn create(mut db: Db, Form(new): Form<New{pascal_name}>) -> AutumnResult<Markup> {{
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
pub async fn edit_form(id: Path<i64>, mut db: Db) -> AutumnResult<Markup> {{
    let row: {pascal_name} = {plural}::table
        .find(*id)
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;
    Ok(layout(&format!("Edit {pascal_name} #{{}}", row.id), html! {{
        h1 {{ "Edit {pascal_name} #" (row.id) }}
        form action=(format!("/{plural}/{{}}/update", row.id)) method="post" {{
{inputs}            button type="submit" {{ "Save" }}
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
    Form(form): Form<New{pascal_name}>,
) -> AutumnResult<Markup> {{
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

fn render_form_inputs(fields: &[Field]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for f in fields {
        let _ = writeln!(
            out,
            "            label {{ \"{name}\" }} input type=\"text\" name=\"{name}\" required;",
            name = f.name
        );
    }
    out
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
        assert!(routes.contains("pub async fn new_form() -> AutumnResult<Markup>"));
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
}
