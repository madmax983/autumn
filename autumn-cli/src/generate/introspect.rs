//! Pure planning for `autumn db pull` — turn an introspected Postgres schema
//! into the same file set the greenfield generators emit: a `#[model]` struct
//! per table, a `diesel::table!` block in `src/schema.rs`, the `pub mod`
//! aggregator line in `src/models/mod.rs`, and (optionally) a
//! `#[repository(Model)]` trait per table.
//!
//! No database access and no clock happen here — the live introspection lives
//! in [`crate::db_pull`]. Keeping this module pure means the type mapping and
//! the emitted file shape are unit-testable without Docker, and the round-trip
//! property (a greenfield-generated table re-derived here is byte-identical)
//! can be asserted directly against [`super::model`].

use std::fmt::Write as _;
use std::path::Path;

use super::dsl::FieldKind;
use super::emit::Plan;
use super::naming::{pascal, snake};
use super::schema_edit::{add_mod_declaration, schema_has_table, singularize, update_main_rs};
use super::{GenerateError, ensure_project_root};

/// A single introspected column, in catalog (`ordinal_position`) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Column name (used verbatim as the struct field and schema column).
    pub name: String,
    /// Mapped field kind (the inverse of the DSL forward map).
    pub kind: FieldKind,
    /// True when the column is nullable (`Option<T>` in the model).
    pub nullable: bool,
    /// True when the column participates in the table's primary key.
    pub is_pk: bool,
}

impl Column {
    /// The Rust type for the model struct, wrapping nullable columns in `Option`.
    #[must_use]
    pub fn rust_type(&self) -> String {
        let inner = self.kind.rust_type();
        if self.nullable {
            format!("Option<{inner}>")
        } else {
            inner.to_owned()
        }
    }

    /// The Diesel `schema.rs` type token, wrapping nullable columns in `Nullable`.
    #[must_use]
    pub fn schema_type(&self) -> String {
        let inner = self.kind.schema_type();
        if self.nullable {
            format!("Nullable<{inner}>")
        } else {
            inner.to_owned()
        }
    }
}

/// An introspected table: its name plus its columns in catalog order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchema {
    /// The table name (as it appears in Postgres, typically plural).
    pub table: String,
    /// Columns in `ordinal_position` order.
    pub columns: Vec<Column>,
}

/// Options controlling what `db pull` emits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PullOptions {
    /// Also emit a `#[repository(Model)]` trait per table.
    pub with_repository: bool,
}

/// Compute every filesystem action an `autumn db pull` would perform.
///
/// Pure planning step — no I/O. The live introspection in [`crate::db_pull`]
/// builds the `tables` and then calls this. Unlike the greenfield generators,
/// **no migration is emitted**: the tables already exist in the database.
///
/// # Errors
/// Returns [`GenerateError::NotInProject`] when `project_root` is not an Autumn
/// project, or surfaces collisions when the model/repository files already exist
/// and `--force` was not given (during [`Plan::execute`]).
pub fn plan_pull(
    project_root: &Path,
    tables: &[TableSchema],
    options: PullOptions,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;

    let mut plan = Plan::new(project_root);

    let models_dir = project_root.join("src").join("models");
    let repos_dir = project_root.join("src").join("repositories");

    // Fold the in-place edits (mod.rs / schema.rs / repositories/mod.rs) across
    // every table into a single computed string each, so multi-table pulls touch
    // each aggregator file exactly once.
    let mut models_mod = read_or_empty(&models_dir.join("mod.rs"));
    let mut schema = read_or_empty(&project_root.join("src").join("schema.rs"));
    let mut repos_mod = read_or_empty(&repos_dir.join("mod.rs"));

    for table in tables {
        let resource = singularize(&table.table);
        let snake_name = snake(&resource);
        let pascal_name = pascal(&resource);

        // (a) src/models/<snake>.rs
        plan.create(
            models_dir.join(format!("{snake_name}.rs")),
            render_model(&pascal_name, &table.table, &table.columns),
        );
        // (b) src/models/mod.rs aggregator line
        models_mod = add_mod_declaration(&models_mod, &snake_name);
        // (c) src/schema.rs table! block
        schema = append_schema_block(&schema, &table.table, &table.columns);

        // (d) optional repository
        if options.with_repository {
            plan.create(
                repos_dir.join(format!("{snake_name}.rs")),
                super::scaffold::render_repository_for_pull(&pascal_name, &snake_name),
            );
            repos_mod = add_mod_declaration(&repos_mod, &snake_name);
        }
    }

    plan.modify(models_dir.join("mod.rs"), models_mod);
    plan.modify(project_root.join("src").join("schema.rs"), schema);

    // src/main.rs: declare the modules the emitted code lives in. A fresh
    // `autumn new` main.rs declares none of these, so without this the
    // generated app would not compile.
    let main_path = project_root.join("src").join("main.rs");
    if let Ok(main_existing) = std::fs::read_to_string(&main_path) {
        let mut mods = vec!["models", "schema"];
        if options.with_repository {
            mods.push("repositories");
        }
        let updated = update_main_rs(&main_existing, &mods, &[]);
        plan.modify(main_path, updated);
    }

    if options.with_repository {
        plan.modify(repos_dir.join("mod.rs"), repos_mod);
    }

    // Cargo.toml: the `#[model]` expansion references diesel/serde/chrono/uuid,
    // none of which a freshly-`autumn new`-ed project carries.
    super::model::plan_cargo_deps(&mut plan, project_root, super::model::MODEL_DEPS);

    Ok(plan)
}

/// Render a `#[model]` struct from introspected columns.
///
/// Columns are emitted in catalog order; the primary-key column is annotated
/// `#[id]` and a `created_at` column is annotated `#[default]` (so it stays out
/// of `NewX`). For a greenfield-generated table — `id` `int8` PK first, user
/// fields, `created_at` last — this is byte-identical to
/// [`super::model::render_model_file`], which the round-trip property relies on.
#[must_use]
pub fn render_model(pascal_name: &str, table: &str, columns: &[Column]) -> String {
    let mut out = String::with_capacity(columns.len() * 64 + 256);
    out.push_str("//! Generated by `autumn generate`.\n");
    out.push_str("//!\n");
    out.push_str("//! Edit this file freely — once a generator has run, the\n");
    out.push_str("//! framework treats this as ordinary user code.\n\n");
    let _ = writeln!(out, "use crate::schema::{table};");
    out.push('\n');
    out.push_str("#[autumn_web::model]\n");
    let _ = writeln!(out, "pub struct {pascal_name} {{");
    for col in columns {
        if col.is_pk {
            out.push_str("    #[id]\n");
        }
        if col.name == "created_at" {
            out.push_str("    #[default]\n");
        }
        let _ = writeln!(out, "    pub {}: {},", col.name, col.rust_type());
    }
    out.push_str("}\n");
    out
}

/// Build a `diesel::table!` block from introspected columns.
#[must_use]
pub fn render_schema_block(table: &str, columns: &[Column]) -> String {
    let pk: Vec<&str> = columns
        .iter()
        .filter(|c| c.is_pk)
        .map(|c| c.name.as_str())
        .collect();
    let pk_clause = if pk.is_empty() {
        // Diesel's `table!` requires a primary key; fall back to the first
        // column when the table declares none (rare; introspected tables
        // tested here always have one).
        columns.first().map_or("id", |c| c.name.as_str()).to_owned()
    } else {
        pk.join(", ")
    };
    let mut out = String::new();
    out.push_str("diesel::table! {\n");
    let _ = writeln!(out, "    {table} ({pk_clause}) {{");
    for col in columns {
        let _ = writeln!(out, "        {} -> {},", col.name, col.schema_type());
    }
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

/// Append a `table!` block to `schema.rs`, idempotently (no-op if `table`
/// already has a block), mirroring [`super::schema_edit::append_schema_table`].
fn append_schema_block(existing: &str, table: &str, columns: &[Column]) -> String {
    if schema_has_table(existing, table) {
        return existing.to_owned();
    }
    let block = render_schema_block(table, columns);
    if existing.is_empty() {
        return block;
    }
    let trimmed = existing.trim_end();
    format!("{trimmed}\n\n{block}")
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::Flags;
    use crate::generate::emit::Action;
    use std::fs;
    use tempfile::TempDir;

    /// A realistic `autumn new` project: Cargo.toml + a main.rs with the
    /// builder chain so module wiring can be exercised.
    fn project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nedition = \"2024\"\n\n[dependencies]\nautumn-web = \"0.5\"\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(
            tmp.path().join("src/main.rs"),
            "use autumn_web::prelude::*;\n\n#[autumn_web::main]\nasync fn main() {\n    autumn_web::app()\n        .routes(routes![])\n        .run()\n        .await;\n}\n",
        )
        .unwrap();
        tmp
    }

    fn col(name: &str, kind: FieldKind, nullable: bool, is_pk: bool) -> Column {
        Column {
            name: name.to_owned(),
            kind,
            nullable,
            is_pk,
        }
    }

    /// The columns a greenfield `Post title:String body:Text published:bool`
    /// table introspects to: `id` int8 PK, the user fields, `created_at` last.
    fn post_table() -> TableSchema {
        TableSchema {
            table: "posts".to_owned(),
            columns: vec![
                col("id", FieldKind::I64, false, true),
                col("title", FieldKind::String, false, false),
                col("body", FieldKind::String, false, false),
                col("published", FieldKind::Bool, false, false),
                col("created_at", FieldKind::NaiveDateTime, false, false),
            ],
        }
    }

    fn rel_paths(plan: &Plan) -> Vec<String> {
        plan.actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(&plan.project_root)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn plan_pull_outside_project_root_errors() {
        let tmp = TempDir::new().unwrap();
        let err = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    #[test]
    fn plan_pull_creates_expected_file_set_and_no_migration() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        let p = rel_paths(&plan);
        assert!(p.contains(&"src/models/post.rs".into()));
        assert!(p.contains(&"src/models/mod.rs".into()));
        assert!(p.contains(&"src/schema.rs".into()));
        assert!(
            p.iter().all(|path| !path.contains("migrations")),
            "db pull must not emit a migration: {p:?}"
        );
    }

    #[test]
    fn plan_pull_model_honors_conventions() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags::default()).unwrap();

        let model = fs::read_to_string(tmp.path().join("src/models/post.rs")).unwrap();
        assert!(model.contains("#[autumn_web::model]"));
        assert!(model.contains("pub struct Post"));
        assert!(model.contains("#[id]"));
        assert!(model.contains("pub id: i64,"), "i64 PK must be preserved");
        assert!(model.contains("pub title: String,"));
        assert!(model.contains("pub published: bool,"));
        assert!(model.contains("#[default]\n    pub created_at: chrono::NaiveDateTime,"));
    }

    #[test]
    fn plan_pull_nullable_columns_become_option() {
        let tmp = project();
        let table = TableSchema {
            table: "notes".to_owned(),
            columns: vec![
                col("id", FieldKind::I64, false, true),
                col("body", FieldKind::String, true, false),
            ],
        };
        let plan = plan_pull(tmp.path(), &[table], PullOptions::default()).unwrap();
        plan.execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/note.rs")).unwrap();
        assert!(
            model.contains("pub body: Option<String>,"),
            "nullable column must be Option<T>: {model}"
        );
        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(
            schema.contains("body -> Nullable<Text>,"),
            "nullable column schema must be Nullable<T>: {schema}"
        );
    }

    #[test]
    fn plan_pull_schema_block_and_mod_line() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags::default()).unwrap();

        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(schema.contains("posts (id)"));
        assert!(schema.contains("title -> Text,"));
        assert!(schema.contains("id -> Int8,"));

        let mod_rs = fs::read_to_string(tmp.path().join("src/models/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod post;"));
    }

    #[test]
    fn plan_pull_wires_modules_into_main_rs() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(
            main.contains("mod models;"),
            "main.rs must declare models: {main}"
        );
        assert!(
            main.contains("mod schema;"),
            "main.rs must declare schema: {main}"
        );
    }

    #[test]
    fn plan_pull_multiple_tables_fold_aggregators_once() {
        let tmp = project();
        let comments = TableSchema {
            table: "comments".to_owned(),
            columns: vec![
                col("id", FieldKind::I64, false, true),
                col("body", FieldKind::String, false, false),
                col("created_at", FieldKind::NaiveDateTime, false, false),
            ],
        };
        let plan = plan_pull(
            tmp.path(),
            &[post_table(), comments],
            PullOptions::default(),
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert_eq!(schema.matches("posts (id)").count(), 1);
        assert_eq!(schema.matches("comments (id)").count(), 1);

        let mod_rs = fs::read_to_string(tmp.path().join("src/models/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod post;"));
        assert!(mod_rs.contains("pub mod comment;"));
        // exactly one Modify per aggregator file.
        let schema_mods = plan
            .actions
            .iter()
            .filter(|a| matches!(a, Action::Modify { path, .. } if path.ends_with("schema.rs")))
            .count();
        assert_eq!(schema_mods, 1);
    }

    #[test]
    fn plan_pull_with_repository_flag() {
        let tmp = project();
        let plan = plan_pull(
            tmp.path(),
            &[post_table()],
            PullOptions {
                with_repository: true,
            },
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let repo = fs::read_to_string(tmp.path().join("src/repositories/post.rs")).unwrap();
        assert!(repo.contains("#[autumn_web::repository(Post, api = \"/api/posts\")]"));
        let repo_mod = fs::read_to_string(tmp.path().join("src/repositories/mod.rs")).unwrap();
        assert!(repo_mod.contains("pub mod post;"));
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(main.contains("mod repositories;"));
    }

    #[test]
    fn plan_pull_without_repository_flag_emits_no_repository() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags::default()).unwrap();
        assert!(!tmp.path().join("src/repositories/post.rs").exists());
    }

    #[test]
    fn plan_pull_collision_without_force_errors() {
        let tmp = project();
        let models = tmp.path().join("src/models");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("post.rs"), "// existing").unwrap();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        let err = plan.execute(Flags::default()).unwrap_err();
        assert!(matches!(err, GenerateError::Collisions(_)));
        // Untouched without --force.
        assert_eq!(
            fs::read_to_string(models.join("post.rs")).unwrap(),
            "// existing"
        );
    }

    #[test]
    fn plan_pull_force_overwrites() {
        let tmp = project();
        let models = tmp.path().join("src/models");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("post.rs"), "// existing").unwrap();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags {
            force: true,
            dry_run: false,
        })
        .unwrap();
        assert!(
            fs::read_to_string(models.join("post.rs"))
                .unwrap()
                .contains("pub struct Post")
        );
    }

    #[test]
    fn plan_pull_dry_run_writes_nothing() {
        let tmp = project();
        let plan = plan_pull(tmp.path(), &[post_table()], PullOptions::default()).unwrap();
        plan.execute(Flags {
            dry_run: true,
            force: false,
        })
        .unwrap();
        assert!(!tmp.path().join("src/models/post.rs").exists());
        assert!(!tmp.path().join("src/schema.rs").exists());
    }

    // ── Round-trip property (AC4) ───────────────────────────────────────────

    #[test]
    fn round_trip_matches_greenfield_model_byte_for_byte() {
        use crate::generate::dsl::{Field, sql_type_to_field_kind};
        use crate::generate::model::render_model_file_for_test;

        // Forward: the model `autumn generate model Post title:String body:Text
        // published:bool` would render.
        let fields = vec![
            Field {
                name: "title".into(),
                kind: FieldKind::String,
                nullable: false,
            },
            Field {
                name: "body".into(),
                kind: FieldKind::Text,
                nullable: false,
            },
            Field {
                name: "published".into(),
                kind: FieldKind::Bool,
                nullable: false,
            },
        ];
        let greenfield = render_model_file_for_test("Post", "posts", &fields);

        // Inverse: synthesize the catalog rows that table produces, invert each
        // udt_name, and re-render via the introspection path.
        let udts = [
            ("id", "int8", false, true),
            ("title", "text", false, false),
            ("body", "text", false, false),
            ("published", "bool", false, false),
            ("created_at", "timestamp", false, false),
        ];
        let columns: Vec<Column> = udts
            .iter()
            .map(|(name, udt, nullable, is_pk)| Column {
                name: (*name).to_owned(),
                kind: sql_type_to_field_kind(udt).unwrap(),
                nullable: *nullable,
                is_pk: *is_pk,
            })
            .collect();
        let re_derived = render_model(&pascal(&singularize("posts")), "posts", &columns);

        assert_eq!(
            greenfield, re_derived,
            "introspected model must be byte-identical to greenfield"
        );
    }
}
