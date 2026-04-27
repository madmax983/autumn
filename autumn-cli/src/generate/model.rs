//! `autumn generate model` — emit a `#[model]` struct, its migration, and a
//! `schema.rs` table block.

use std::path::Path;

use super::dsl::{Field, parse_fields};
use super::emit::Plan;
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::{
    add_mod_declaration, append_schema_table, create_table_sql, drop_table_sql,
};
use super::{Flags, GenerateError, ensure_project_root, timestamp_now};

/// Compute every action a `generate model` invocation would perform.
///
/// Pure planning step — no I/O happens here. Tests use this directly so they
/// can inspect the emitted file list and contents without touching the disk.
///
/// # Errors
/// Surfaces project-layout, DSL, and naming errors before any file is written.
pub fn plan_model(
    project_root: &Path,
    name: &str,
    field_tokens: &[String],
    timestamp: &str,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;
    validate_resource_name(name)?;
    let fields = parse_fields(field_tokens)?;
    validate_field_names(&fields)?;

    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let table = pluralize(&snake_name);

    let mut plan = Plan::new(project_root);

    // (a) `src/models/<snake>.rs` + `src/models/mod.rs`
    let models_dir = project_root.join("src").join("models");
    let model_file = models_dir.join(format!("{snake_name}.rs"));
    plan.create(model_file, render_model_file(&pascal_name, &table, &fields));

    let mod_path = models_dir.join("mod.rs");
    let mod_existing = read_or_empty(&mod_path);
    plan.modify(mod_path, add_mod_declaration(&mod_existing, &snake_name));

    // (b) Diesel migration
    let migration_dir_name = format!("{timestamp}_create_{table}");
    let migration_dir = project_root.join("migrations").join(&migration_dir_name);
    plan.create(
        migration_dir.join("up.sql"),
        create_table_sql(&table, &fields),
    );
    plan.create(migration_dir.join("down.sql"), drop_table_sql(&table));

    // (c) `src/schema.rs` entry
    let schema_path = project_root.join("src").join("schema.rs");
    let schema_existing = read_or_empty(&schema_path);
    plan.modify(
        schema_path,
        append_schema_table(&schema_existing, &table, &fields),
    );

    // (d) `Cargo.toml` deps — `#[autumn_web::model]` expands to references
    // for `diesel`, `serde`, and `chrono`, none of which are in the
    // freshly-`autumn new`-ed project.
    plan_cargo_deps(&mut plan, project_root, MODEL_DEPS);

    Ok(plan)
}

/// Direct dependencies the *model* generator's output requires at compile time.
pub(super) const MODEL_DEPS: &[(&str, &str)] = &[
    ("chrono", "{ version = \"0.4\", features = [\"serde\"] }"),
    (
        "diesel",
        "{ version = \"2\", features = [\"postgres\", \"chrono\"] }",
    ),
    (
        "diesel-async",
        "{ version = \"0.8\", features = [\"postgres\"] }",
    ),
    (
        "pq-sys",
        "{ version = \"0.7\", features = [\"bundled_without_openssl\"] }",
    ),
    ("diesel_migrations", "\"2\""),
    ("serde", "{ version = \"1\", features = [\"derive\"] }"),
];

/// Append a `Modify` action to `plan` that ensures every `(crate, version_spec)`
/// in `deps` is present under `[dependencies]` in the project's `Cargo.toml`.
/// Existing entries are left untouched.
pub(super) fn plan_cargo_deps(plan: &mut Plan, project_root: &Path, deps: &[(&str, &str)]) {
    let cargo_toml_path = project_root.join("Cargo.toml");
    let existing = read_or_empty(&cargo_toml_path);
    let updated = ensure_cargo_dependencies(&existing, deps);
    if updated != existing {
        plan.modify(cargo_toml_path, updated);
    }
}

/// Insert each `(crate, version_spec)` pair at the end of the `[dependencies]`
/// section, skipping entries already present. Pure string transformation —
/// preserves the rest of the file as-is. If the file has no `[dependencies]`
/// section yet, appends a new one with the requested entries.
fn ensure_cargo_dependencies(existing: &str, deps: &[(&str, &str)]) -> String {
    let lines: Vec<&str> = existing.lines().collect();

    // Locate the `[dependencies]` table header. Tolerate trailing whitespace
    // and `# comments` after the header (`[dependencies] # shared deps`).
    let Some(deps_idx) = lines
        .iter()
        .position(|l| is_table_header(l, "dependencies"))
    else {
        // No `[dependencies]` section yet — append one with all requested deps.
        use std::fmt::Write as _;
        let mut out = String::with_capacity(existing.len() + 64);
        out.push_str(existing);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("[dependencies]\n");
        for (name, spec) in deps {
            let _ = writeln!(out, "{name} = {spec}");
        }
        return out;
    };

    // Find the next `[…]` table header (or the end of file).
    let next_section = lines[deps_idx + 1..]
        .iter()
        .position(|l| is_any_table_header(l))
        .map_or(lines.len(), |off| deps_idx + 1 + off);

    let dep_section = &lines[deps_idx + 1..next_section];

    let to_add: Vec<(&str, &str)> = deps
        .iter()
        .copied()
        .filter(|(name, _)| !dep_section_has(dep_section, name))
        .collect();
    if to_add.is_empty() {
        return existing.to_owned();
    }

    // Drop trailing blank lines from the dep section so the insertion sits
    // flush against the existing entries.
    let mut insert_at = next_section;
    while insert_at > deps_idx + 1 && lines[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }

    let inserted: Vec<String> = to_add
        .iter()
        .map(|(name, spec)| format!("{name} = {spec}"))
        .collect();

    let mut out = String::with_capacity(
        existing.len() + inserted.iter().map(String::len).sum::<usize>() + 16,
    );
    for line in &lines[..insert_at] {
        out.push_str(line);
        out.push('\n');
    }
    for entry in &inserted {
        out.push_str(entry);
        out.push('\n');
    }
    for line in &lines[insert_at..] {
        out.push_str(line);
        out.push('\n');
    }
    // Preserve whether the original file ended with a newline.
    if !existing.ends_with('\n') {
        out.pop();
    }
    out
}

/// True iff `line` is a TOML table header for `[<table>]`, tolerating leading
/// whitespace and trailing `# comment` text (which `cargo` itself accepts).
fn is_table_header(line: &str, table: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    let Some(close_idx) = rest.find(']') else {
        return false;
    };
    if rest[..close_idx].trim() != table {
        return false;
    }
    // Anything after `]` must be whitespace or a `#` comment.
    let after = rest[close_idx + 1..].trim_start();
    after.is_empty() || after.starts_with('#')
}

/// True iff `line` is *any* `[<table>]` header, regardless of the table name.
fn is_any_table_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    let Some(close_idx) = rest.find(']') else {
        return false;
    };
    if rest[..close_idx].trim().is_empty() {
        return false;
    }
    let after = rest[close_idx + 1..].trim_start();
    after.is_empty() || after.starts_with('#')
}

/// True iff `dep_section` contains a line declaring `crate_name = …`.
fn dep_section_has(dep_section: &[&str], crate_name: &str) -> bool {
    dep_section.iter().any(|l| {
        let t = l.trim_start();
        // Strip leading `#` so commented-out lines don't count.
        if t.starts_with('#') {
            return false;
        }
        t.split_once('=')
            .is_some_and(|(name, _)| name.trim() == crate_name)
    })
}

/// Reserved resource names whose snake-case form would collide with a special
/// file in the generated layout (e.g. `mod` → `src/models/mod.rs`).
const RESERVED_RESOURCE_NAMES: &[&str] = &["main", "lib"];

/// Validate a resource name is a non-empty `PascalCase` or `snake_case` identifier.
pub(super) fn validate_resource_name(name: &str) -> Result<(), GenerateError> {
    if name.is_empty() {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            "name cannot be empty".into(),
        ));
    }
    let first = name.chars().next().expect("non-empty");
    if !first.is_ascii_alphabetic() {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            "must start with a letter".into(),
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && *c != '_')
    {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            format!("contains invalid character '{bad}'"),
        ));
    }
    let snake_name = super::naming::snake(name);
    // Snake-case form is used as a module name (`pub mod <snake_name>;`) and as
    // a `crate::models::<snake_name>::…` import path. Rust keywords like `type`,
    // `match`, and `mod` would emit syntactically invalid code.
    if super::dsl::is_rust_keyword(&snake_name) {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            format!(
                "'{name}' is a Rust keyword (its snake_case form '{snake_name}' cannot be a module name)"
            ),
        ));
    }
    if RESERVED_RESOURCE_NAMES.contains(&snake_name.as_str()) {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            format!(
                "'{name}' is reserved — its snake_case form ('{snake_name}') collides with a special file"
            ),
        ));
    }
    Ok(())
}

/// Field names the model template emits unconditionally (`id` and
/// `created_at`). User-provided fields with these names would produce
/// duplicate struct members and duplicate columns in the migration.
const RESERVED_FIELD_NAMES: &[&str] = &["id", "created_at"];

/// Reject user fields whose name collides with a column the template always
/// emits.
fn validate_field_names(fields: &[Field]) -> Result<(), GenerateError> {
    for f in fields {
        if RESERVED_FIELD_NAMES.contains(&f.name.as_str()) {
            return Err(GenerateError::InvalidField {
                token: format!("{}:{}", f.name, f.rust_type()),
                reason: format!(
                    "'{}' is reserved — the generator always emits this column",
                    f.name
                ),
            });
        }
    }
    Ok(())
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn render_model_file(name: &str, table: &str, fields: &[Field]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("//! Generated by `autumn generate`.\n");
    out.push_str("//!\n");
    out.push_str("//! Edit this file freely — once a generator has run, the\n");
    out.push_str("//! framework treats this as ordinary user code.\n\n");
    let _ = writeln!(out, "use crate::schema::{table};");
    out.push('\n');
    out.push_str("#[autumn_web::model]\n");
    let _ = writeln!(out, "pub struct {name} {{");
    out.push_str("    #[id]\n");
    out.push_str("    pub id: i64,\n");
    for f in fields {
        let _ = writeln!(out, "    pub {}: {},", f.name, f.rust_type());
    }
    out.push_str("    #[default]\n");
    out.push_str("    pub created_at: chrono::NaiveDateTime,\n");
    out.push_str("}\n");
    out
}

/// CLI entry point — plan and execute, exiting nonzero on any error.
pub fn run(name: &str, field_tokens: &[String], flags: Flags) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let timestamp = timestamp_now();
    match plan_model(&cwd, name, field_tokens, &timestamp).and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::emit::Action;
    use std::fs;
    use tempfile::TempDir;

    fn project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        tmp
    }

    fn paths(plan: &Plan) -> Vec<String> {
        plan.actions
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
            .collect()
    }

    #[test]
    fn plan_creates_expected_file_set() {
        let tmp = project();
        let plan = plan_model(
            tmp.path(),
            "Post",
            &["title:String".into(), "body:Text".into()],
            "20260427000000",
        )
        .unwrap();
        let p = paths(&plan);
        assert!(p.contains(&"src/models/post.rs".into()));
        assert!(p.contains(&"src/models/mod.rs".into()));
        assert!(p.contains(&"migrations/20260427000000_create_posts/up.sql".into()));
        assert!(p.contains(&"migrations/20260427000000_create_posts/down.sql".into()));
        assert!(p.contains(&"src/schema.rs".into()));
    }

    #[test]
    fn plan_rejects_lowercase_first_char() {
        let tmp = project();
        let err = plan_model(tmp.path(), "123Bad", &[], "20260427000000").unwrap_err();
        assert!(matches!(err, GenerateError::InvalidName(_, _)));
    }

    #[test]
    fn plan_rejects_reserved_resource_names() {
        // Without this guard, `mod` → `src/models/mod.rs` would silently
        // overwrite the per-resource model file with the mod aggregator.
        for name in ["mod", "main", "lib", "Mod"] {
            let tmp = project();
            let err = plan_model(tmp.path(), name, &[], "20260427000000").unwrap_err();
            assert!(
                matches!(err, GenerateError::InvalidName(_, _)),
                "expected '{name}' to be rejected"
            );
        }
    }

    #[test]
    fn plan_rejects_keyword_resource_names() {
        // `Type` → `mod type;` is invalid Rust syntax without raw idents.
        for name in ["Type", "type", "Match", "match", "Self", "Trait"] {
            let tmp = project();
            let err = plan_model(tmp.path(), name, &[], "20260427000000").unwrap_err();
            assert!(
                matches!(err, GenerateError::InvalidName(_, _)),
                "expected '{name}' to be rejected as a keyword"
            );
            assert!(
                err.to_string().contains("keyword"),
                "expected keyword error for '{name}'; got: {err}"
            );
        }
    }

    #[test]
    fn plan_rejects_id_or_created_at_as_user_field() {
        // The model template always emits `id` and `created_at`. Letting the
        // user re-declare them would produce duplicate struct members and
        // duplicate SQL columns.
        for token in ["id:i64", "created_at:NaiveDateTime"] {
            let tmp = project();
            let err =
                plan_model(tmp.path(), "Post", &[token.into()], "20260427000000").unwrap_err();
            assert!(
                matches!(err, GenerateError::InvalidField { .. }),
                "expected '{token}' to be rejected"
            );
            assert!(
                err.to_string().contains("reserved"),
                "expected reserved-field error for '{token}'; got: {err}"
            );
        }
    }

    #[test]
    fn plan_rejects_unsupported_field_type() {
        let tmp = project();
        let err = plan_model(
            tmp.path(),
            "Post",
            &["price:Decimal".into()],
            "20260427000000",
        )
        .unwrap_err();
        assert!(matches!(err, GenerateError::InvalidField { .. }));
    }

    #[test]
    fn plan_outside_project_root_errors() {
        let tmp = TempDir::new().unwrap();
        let err = plan_model(tmp.path(), "Post", &[], "20260427000000").unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    #[test]
    fn execute_writes_idiomatic_model() {
        let tmp = project();
        let plan = plan_model(
            tmp.path(),
            "Post",
            &["title:String".into(), "published:bool".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        let model = fs::read_to_string(tmp.path().join("src/models/post.rs")).unwrap();
        assert!(model.contains("#[autumn_web::model]"));
        assert!(model.contains("pub struct Post"));
        assert!(model.contains("pub title: String,"));
        assert!(model.contains("pub published: bool,"));
        assert!(model.contains("#[id]"));
        assert!(model.contains("pub id: i64,"));
        assert!(model.contains("created_at: chrono::NaiveDateTime"));

        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260427000000_create_posts/up.sql"),
        )
        .unwrap();
        assert!(up.contains("CREATE TABLE posts ("));
        assert!(up.contains("title TEXT NOT NULL"));
        assert!(up.contains("published BOOLEAN NOT NULL"));
        assert!(up.contains("id BIGSERIAL PRIMARY KEY"));

        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(schema.contains("posts (id)"));
        assert!(schema.contains("title -> Text,"));
    }

    #[test]
    fn rerunning_with_force_overwrites_model_but_appends_schema() {
        let tmp = project();
        let plan = plan_model(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();

        // Second run: same model, --force.
        let plan2 = plan_model(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427100000",
        )
        .unwrap();
        plan2
            .execute(Flags {
                force: true,
                dry_run: false,
            })
            .unwrap();

        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        // Only one `posts (id)` block — append is idempotent.
        assert_eq!(schema.matches("posts (id)").count(), 1);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = project();
        let plan = plan_model(tmp.path(), "Post", &[], "20260427000000").unwrap();
        plan.execute(Flags {
            dry_run: true,
            force: false,
        })
        .unwrap();
        assert!(!tmp.path().join("src/models/post.rs").exists());
        assert!(!tmp.path().join("src/schema.rs").exists());
    }

    #[test]
    fn collision_reports_clean_path() {
        let tmp = project();
        // Pre-create the file so the next run collides.
        let dir = tmp.path().join("src/models");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("post.rs"), "// existing").unwrap();
        let plan = plan_model(tmp.path(), "Post", &[], "20260427000000").unwrap();
        let err = plan.execute(Flags::default()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("post.rs"));
    }

    #[test]
    fn modify_actions_marked_correctly() {
        let tmp = project();
        let plan = plan_model(tmp.path(), "Post", &[], "20260427000000").unwrap();
        let modify_count = plan
            .actions
            .iter()
            .filter(|a| matches!(a, Action::Modify { .. }))
            .count();
        // mod.rs, schema.rs, and Cargo.toml are always Modify.
        assert!(modify_count >= 3);
    }

    #[test]
    fn ensure_cargo_dependencies_appends_missing() {
        let original = "[package]\n\
name = \"x\"\n\
\n\
[dependencies]\n\
autumn-web = \"0.3\"\n";
        let updated = ensure_cargo_dependencies(
            original,
            &[
                ("chrono", "\"0.4\""),
                ("autumn-web", "\"99\""), // already present — must not duplicate
            ],
        );
        assert!(updated.contains("autumn-web = \"0.3\""));
        assert!(updated.contains("chrono = \"0.4\""));
        assert_eq!(updated.matches("autumn-web =").count(), 1);
    }

    #[test]
    fn ensure_cargo_dependencies_idempotent() {
        let original = "[package]\nname = \"x\"\n\n[dependencies]\nchrono = \"0.4\"\n";
        let once = ensure_cargo_dependencies(original, &[("chrono", "\"0.4\"")]);
        let twice = ensure_cargo_dependencies(&once, &[("chrono", "\"0.4\"")]);
        assert_eq!(once, twice);
        assert_eq!(once, original);
    }

    #[test]
    fn ensure_cargo_dependencies_inserts_before_next_section() {
        let original = "[package]\nname = \"x\"\n\n\
[dependencies]\nautumn-web = \"0.3\"\n\n\
[dev-dependencies]\ntempfile = \"3\"\n";
        let updated = ensure_cargo_dependencies(original, &[("chrono", "\"0.4\"")]);
        let chrono_pos = updated.find("chrono = \"0.4\"").unwrap();
        let dev_deps_pos = updated.find("[dev-dependencies]").unwrap();
        assert!(
            chrono_pos < dev_deps_pos,
            "chrono must land in [dependencies], not [dev-dependencies]"
        );
    }

    #[test]
    fn ensure_cargo_dependencies_skips_commented_out_entries() {
        let original = "[dependencies]\n# autumn-web = \"0.2\"\n";
        let updated = ensure_cargo_dependencies(original, &[("autumn-web", "\"0.3\"")]);
        assert!(updated.contains("autumn-web = \"0.3\""));
    }

    #[test]
    fn ensure_cargo_dependencies_handles_header_with_trailing_comment() {
        // `[dependencies] # shared deps` is valid TOML — must not be treated
        // as a missing section.
        let original = "[dependencies] # shared deps\nautumn-web = \"0.3\"\n";
        let updated = ensure_cargo_dependencies(original, &[("chrono", "\"0.4\"")]);
        // No second `[dependencies]` table appended.
        assert_eq!(
            updated.matches("[dependencies]").count(),
            1,
            "duplicate [dependencies] table appended:\n{updated}"
        );
        assert!(updated.contains("chrono = \"0.4\""));
    }

    #[test]
    fn ensure_cargo_dependencies_treats_indented_section_as_a_header() {
        // Indented headers are accepted by cargo and our scanner mustn't
        // treat them as bare dep entries.
        let original = "[package]\nname = \"x\"\n\n[dependencies]\nautumn-web = \"0.3\"\n\n  [dev-dependencies]\ntempfile = \"3\"\n";
        let updated = ensure_cargo_dependencies(original, &[("chrono", "\"0.4\"")]);
        let chrono_pos = updated.find("chrono = \"0.4\"").unwrap();
        let dev_deps_pos = updated.find("[dev-dependencies]").unwrap();
        assert!(
            chrono_pos < dev_deps_pos,
            "chrono must land in [dependencies], not [dev-dependencies]"
        );
    }

    #[test]
    fn plan_includes_cargo_toml_modification() {
        let tmp = project();
        let plan = plan_model(tmp.path(), "Post", &[], "20260427000000").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("Cargo.toml")),
            "plan must touch Cargo.toml so generated code compiles"
        );
    }

    #[test]
    fn execute_adds_chrono_and_diesel_to_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        // Realistic `autumn new` Cargo.toml.
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nedition = \"2024\"\n\n\
[dependencies]\nautumn-web = \"0.3\"\n",
        )
        .unwrap();
        let plan = plan_model(
            tmp.path(),
            "Post",
            &["title:String".into()],
            "20260427000000",
        )
        .unwrap();
        plan.execute(Flags::default()).unwrap();
        let cargo_toml = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        for dep in [
            "chrono",
            "diesel",
            "diesel-async",
            "serde",
            "diesel_migrations",
        ] {
            assert!(
                cargo_toml.contains(&format!("{dep} =")),
                "missing '{dep}' in Cargo.toml after `generate model`:\n{cargo_toml}"
            );
        }
    }
}
