//! Project scaffolding for `autumn new <name>`.
//!
//! Generates a complete Autumn project directory from embedded templates.

use std::fs;
use std::io::Write;
use std::path::Path;

use autumn_web::credentials::{MasterKey, encrypt};

mod templates {
    pub const CARGO_TOML: &str = include_str!("templates/Cargo.toml.tmpl");
    pub const MAIN_RS: &str = include_str!("templates/main.rs.tmpl");
    pub const AUTUMN_TOML: &str = include_str!("templates/autumn.toml.tmpl");
    pub const DOCKERFILE: &str = include_str!("templates/Dockerfile.tmpl");
    pub const DOCKERIGNORE: &str = include_str!("templates/.dockerignore.tmpl");
    pub const BUILD_RS: &str = include_str!("templates/build.rs.tmpl");
    pub const INPUT_CSS: &str = include_str!("templates/input.css.tmpl");
    pub const TAILWIND_CONFIG: &str = include_str!("templates/tailwind.config.js.tmpl");
    pub const GITIGNORE: &str = include_str!("templates/gitignore.tmpl");
    pub const SEED_RS: &str = include_str!("templates/seed.rs.tmpl");
    pub const SEED_CARGO_TOML: &str = include_str!("templates/seed_Cargo.toml.tmpl");
    pub const INTEGRATION_TEST: &str = include_str!("templates/tests/integration_test.rs.tmpl");
}

/// Errors that can occur during project generation.
#[derive(Debug, thiserror::Error)]
pub enum NewError {
    /// The project name is not a valid Rust package name.
    #[error("invalid project name '{0}': {1}")]
    InvalidName(String, String),

    /// A directory with this name already exists.
    #[error("directory '{0}' already exists")]
    AlreadyExists(String),

    /// Filesystem error during project creation.
    #[error("failed to create project: {0}")]
    Io(#[from] std::io::Error),
}

/// Entry point called from `main.rs` and delegates to [`generate_with`].
pub fn run(name: &str, opts: GenerateOptions) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine current directory: {e}");
        std::process::exit(1);
    });
    let result = if opts == GenerateOptions::default() {
        generate(name, &cwd)
    } else {
        generate_with(name, &cwd, opts)
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Optional toggles applied to project generation.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct GenerateOptions {
    /// Scaffold the optional i18n module (`i18n/en.ftl`, `[i18n]` block,
    /// `i18n` feature flag on `autumn-web`).
    pub with_i18n: bool,
    /// Scaffold the optional seed binary and enable `autumn-web/seed`.
    pub with_seed: bool,
    /// Daemon-flavored starter: a model-free app that builds with **no** Postgres
    /// (drops the default `db` feature and migrations), ready for `autumn serve`.
    pub with_daemon: bool,
    /// Managed/bundled-Postgres daemon starter: keeps `db`, enables the
    /// `managed-pg` feature, and wires a managed local Postgres provider.
    /// Implies [`Self::with_daemon`]-style serve usage. Mutually exclusive with a
    /// DB-free daemon.
    pub with_bundled_pg: bool,
}

/// Generate a new Autumn project under `parent_dir/name` with default options.
pub fn generate(name: &str, parent_dir: &Path) -> Result<(), NewError> {
    generate_with(name, parent_dir, GenerateOptions::default())
}

/// Generate a new Autumn project under `parent_dir/name`, honouring `opts`.
pub fn generate_with(name: &str, parent_dir: &Path, opts: GenerateOptions) -> Result<(), NewError> {
    validate_name(name)?;

    let project_dir = parent_dir.join(name);
    if project_dir.exists() {
        return Err(NewError::AlreadyExists(name.to_owned()));
    }

    let crate_name = name.replace('-', "_");
    let autumn_version = env!("CARGO_PKG_VERSION");
    let rust_version = option_env!("CARGO_PKG_RUST_VERSION").unwrap_or("1.88.0");

    fs::create_dir_all(project_dir.join("src"))?;
    fs::create_dir_all(project_dir.join("static/css"))?;
    fs::create_dir_all(project_dir.join("migrations"))?;
    fs::create_dir_all(project_dir.join("tests"))?;
    fs::create_dir_all(project_dir.join("config/credentials"))?;
    if opts.with_i18n {
        fs::create_dir_all(project_dir.join("i18n"))?;
    }

    let render = |template: &str| -> String {
        template
            .replace("{{project_name}}", name)
            .replace("{{crate_name}}", &crate_name)
            .replace("{{autumn_version}}", autumn_version)
            .replace("{{rust_version}}", rust_version)
    };

    let cargo_toml = render_cargo_toml(
        opts,
        autumn_version,
        render(templates::CARGO_TOML),
        &render(templates::SEED_CARGO_TOML),
    );
    fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

    let mut main_rs = if opts.with_i18n {
        render(templates::MAIN_RS).replace(
            "        .routes(routes![index, hello, hello_name])",
            "        .i18n_auto()\n        .routes(routes![index, hello, hello_name])",
        )
    } else {
        render(templates::MAIN_RS)
    };
    if opts.with_daemon && !opts.with_bundled_pg {
        // DB-free daemon: the `migrate` module is db-gated, so drop migrations.
        main_rs = strip_migrations(&main_rs);
    }
    fs::write(project_dir.join("src/main.rs"), main_rs)?;

    let mut autumn_toml = if opts.with_i18n {
        let mut s = render(templates::AUTUMN_TOML);
        s.push_str("\n[i18n]\ndefault_locale = \"en\"\nsupported_locales = [\"en\"]\n");
        s
    } else {
        render(templates::AUTUMN_TOML)
    };
    if opts.with_daemon && !opts.with_bundled_pg {
        autumn_toml.push_str(
            "\n# Daemon starter: this app uses no database. Run it as a local\n\
             # daemon with `autumn serve --daemon` (no Postgres required).\n",
        );
    }
    fs::write(project_dir.join("autumn.toml"), autumn_toml)?;
    fs::write(
        project_dir.join("Dockerfile"),
        render(templates::DOCKERFILE),
    )?;
    fs::write(
        project_dir.join(".dockerignore"),
        render(templates::DOCKERIGNORE),
    )?;
    fs::write(project_dir.join("build.rs"), render(templates::BUILD_RS))?;
    fs::write(
        project_dir.join("static/css/input.css"),
        render(templates::INPUT_CSS),
    )?;
    fs::write(
        project_dir.join("tailwind.config.js"),
        render(templates::TAILWIND_CONFIG),
    )?;
    fs::write(project_dir.join(".gitignore"), render(templates::GITIGNORE))?;
    fs::write(project_dir.join("migrations/.gitkeep"), "")?;

    scaffold_credentials(&project_dir, name)?;
    fs::write(
        project_dir.join("tests/integration_test.rs"),
        render(templates::INTEGRATION_TEST),
    )?;

    write_optional_scaffold_files(&project_dir, name, opts, &render)?;

    print_scaffold_summary(name, opts);

    Ok(())
}

fn scaffold_credentials(project_dir: &Path, name: &str) -> Result<(), NewError> {
    let master_key = MasterKey::generate();
    let key_path = project_dir.join("config/master.key");

    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&key_path)?;
    f.write_all(master_key.to_hex().as_bytes())?;

    let template = format!(
        "# Encrypted credentials for '{name}'\n\
         # Run `autumn credentials edit` to update these values.\n\
         # Do NOT commit config/master.key to version control.\n\
         \n\
         # stripe_secret_key = \"sk_live_...\"\n\
         # sendgrid_api_key = \"SG...\"\n\
         # s3_access_key_id = \"AKIA...\"\n"
    );
    let ciphertext = encrypt(&master_key, template.as_bytes());
    fs::write(
        project_dir.join("config/credentials/development.toml.enc"),
        ciphertext,
    )?;

    Ok(())
}

fn print_scaffold_summary(name: &str, opts: GenerateOptions) {
    println!("  Created {name}/");
    println!("  Created {name}/Cargo.toml");
    println!("  Created {name}/autumn.toml");
    println!("  Created {name}/Dockerfile");
    println!("  Created {name}/.dockerignore");
    println!("  Created {name}/build.rs");
    println!("  Created {name}/src/main.rs");
    if opts.with_seed {
        println!("  Created {name}/src/bin/seed.rs");
    }
    println!("  Created {name}/static/css/input.css");
    println!("  Created {name}/tailwind.config.js");
    println!("  Created {name}/.gitignore");
    println!("  Created {name}/migrations/");
    println!("  Created {name}/tests/integration_test.rs");
    println!("  Created {name}/config/master.key (keep secret — never commit)");
    println!("  Created {name}/config/credentials/development.toml.enc");
    if opts.with_i18n {
        println!("  Created {name}/i18n/en.ftl");
    }
    println!();
    println!("Get started:");
    println!("  cd {name}");
    println!("  cargo run");
    println!();
    println!("Your app will be available at http://localhost:3000");
    if opts.with_i18n {
        println!();
        println!("i18n: edit i18n/en.ftl, add more locales as i18n/<tag>.ftl,");
        println!("      and use the t!() macro in handlers — see docs/guide/i18n.md.");
    }
    if opts.with_seed {
        println!();
        println!("Seed your database:");
        println!("  autumn migrate && autumn seed");
    }
}

/// Remove the diesel-migrations wiring from a generated `main.rs` so a DB-free
/// app compiles without the db-gated `migrate` module.
fn strip_migrations(main_rs: &str) -> String {
    main_rs
        .replace(
            "use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};\n",
            "",
        )
        .replace(
            "const MIGRATIONS: EmbeddedMigrations = embed_migrations!();\n\n",
            "",
        )
        .replace("\n        .migrations(MIGRATIONS)", "")
}

/// Default `autumn-web` features minus `db` — the DB-free daemon feature set.
const DAEMON_NO_DB_FEATURES: &[&str] = &[
    "maud",
    "htmx",
    "tailwind",
    "cache-moka",
    "http-client",
    "reporting",
];

fn render_cargo_toml(
    opts: GenerateOptions,
    autumn_version: &str,
    mut cargo_toml: String,
    seed_bin_toml: &str,
) -> String {
    use std::fmt::Write;

    // DB-free daemon starter: switch off default features (drops `db`) so the
    // binary links no Postgres, and remove the diesel migrations dependency.
    if opts.with_daemon && !opts.with_bundled_pg {
        let plain_dep = format!(r#"autumn-web = "{autumn_version}""#);
        let mut features: Vec<&str> = DAEMON_NO_DB_FEATURES.to_vec();
        if opts.with_i18n {
            features.push("i18n");
        }
        let features_str = features
            .iter()
            .map(|f| format!(r#""{f}""#))
            .collect::<Vec<_>>()
            .join(", ");
        let dep = format!(
            r#"autumn-web = {{ version = "{autumn_version}", default-features = false, features = [{features_str}] }}"#
        );
        cargo_toml = cargo_toml.replace(&plain_dep, &dep);
        cargo_toml = cargo_toml.replace("diesel_migrations = \"2\"\n", "");
        return cargo_toml;
    }

    let mut features = Vec::new();
    if opts.with_i18n {
        features.push("i18n");
    }
    if opts.with_seed {
        features.push("seed");
    }
    if !features.is_empty() {
        let plain_dep = format!(r#"autumn-web = "{autumn_version}""#);
        // ⚡ Bolt optimization: Pre-allocate capacity for comma-separated feature strings
        let mut features_str = String::with_capacity(features.len() * 10);
        for (i, feature) in features.iter().enumerate() {
            if i > 0 {
                features_str.push_str(", ");
            }
            write!(features_str, r#""{feature}""#).unwrap();
        }
        let feature_dep = format!(
            r#"autumn-web = {{ version = "{autumn_version}", features = [{features_str}] }}"#
        );
        cargo_toml = cargo_toml.replace(&plain_dep, &feature_dep);
    }
    if opts.with_seed {
        cargo_toml.push('\n');
        cargo_toml.push_str(seed_bin_toml);
    }
    cargo_toml
}

fn write_optional_scaffold_files(
    project_dir: &Path,
    name: &str,
    opts: GenerateOptions,
    render: &impl Fn(&str) -> String,
) -> Result<(), NewError> {
    if opts.with_i18n {
        fs::write(
            project_dir.join("i18n/en.ftl"),
            "# Default-locale translations for {{project_name}}.\n\
             # Add more locales by dropping additional files like `i18n/es.ftl`.\n\
             welcome.title = Welcome to Autumn!\n\
             welcome.greeting = Hello, { $name }!\n"
                .replace("{{project_name}}", name),
        )?;
    }

    if opts.with_seed {
        fs::create_dir_all(project_dir.join("src/bin"))?;
        fs::write(
            project_dir.join("src/bin/seed.rs"),
            render(templates::SEED_RS),
        )?;
    }

    Ok(())
}

/// Rust keywords that would be invalid crate names.
const KEYWORDS: &[&str] = &[
    "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Validate that a name is a valid Rust package name.
fn validate_name(name: &str) -> Result<(), NewError> {
    if name.is_empty() {
        return Err(NewError::InvalidName(
            name.to_owned(),
            "name cannot be empty".into(),
        ));
    }

    let first = name.chars().next().expect("checked non-empty");
    if !first.is_ascii_alphabetic() {
        return Err(NewError::InvalidName(
            name.to_owned(),
            "must start with a letter".into(),
        ));
    }

    if let Some(bad) = name
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && *c != '-' && *c != '_')
    {
        return Err(NewError::InvalidName(
            name.to_owned(),
            format!("contains invalid character '{bad}'"),
        ));
    }

    if KEYWORDS.contains(&name) {
        return Err(NewError::InvalidName(
            name.to_owned(),
            format!("'{name}' is a Rust keyword"),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn valid_name_simple() {
        assert!(validate_name("myapp").is_ok());
    }

    #[test]
    fn valid_name_with_hyphens() {
        assert!(validate_name("my-app").is_ok());
    }

    #[test]
    fn valid_name_with_underscores() {
        assert!(validate_name("my_app").is_ok());
    }

    #[test]
    fn valid_name_with_digits() {
        assert!(validate_name("app2").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        let err = validate_name("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn starts_with_digit_rejected() {
        let err = validate_name("3app").unwrap_err();
        assert!(err.to_string().contains("start with a letter"));
    }

    #[test]
    fn starts_with_hyphen_rejected() {
        let err = validate_name("-app").unwrap_err();
        assert!(err.to_string().contains("start with a letter"));
    }

    #[test]
    fn special_chars_rejected() {
        let err = validate_name("my app!").unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn keyword_rejected() {
        let err = validate_name("fn").unwrap_err();
        assert!(err.to_string().contains("keyword"));
    }

    #[test]
    fn keyword_async_rejected() {
        let err = validate_name("async").unwrap_err();
        assert!(err.to_string().contains("keyword"));
    }

    #[test]
    fn generates_all_expected_files() {
        let tmp = TempDir::new().unwrap();
        generate("test-app", tmp.path()).unwrap();

        let p = tmp.path().join("test-app");
        assert!(p.join("Cargo.toml").is_file());
        assert!(p.join("src/main.rs").is_file());
        assert!(p.join("autumn.toml").is_file());
        assert!(p.join("Dockerfile").is_file());
        assert!(p.join(".dockerignore").is_file());
        let dockerignore = std::fs::read_to_string(p.join(".dockerignore")).unwrap();
        assert!(
            dockerignore.contains("/config/master.key")
                || dockerignore.contains("config/master.key"),
            ".dockerignore must exclude config/master.key: {dockerignore}"
        );
        assert!(p.join("build.rs").is_file());
        assert!(p.join(".gitignore").is_file());
        assert!(p.join("static/css/input.css").is_file());
        assert!(p.join("tailwind.config.js").is_file());
        assert!(p.join("migrations/.gitkeep").is_file());
        assert!(!p.join("src/lib.rs").exists());
        assert!(!p.join("src/client.rs").exists());
    }

    // `autumn new` must generate a tests/ directory with a smoke test.
    #[test]
    fn generates_tests_directory_with_smoke_test() {
        let tmp = TempDir::new().unwrap();
        generate("smoke-test-app", tmp.path()).unwrap();
        let p = tmp.path().join("smoke-test-app");
        assert!(
            p.join("tests").is_dir(),
            "`autumn new` should create a tests/ directory"
        );
        assert!(
            p.join("tests/integration_test.rs").is_file(),
            "`autumn new` should generate tests/integration_test.rs"
        );
    }

    // The generated Cargo.toml must have [dev-dependencies] with tokio
    // so that #[tokio::test] compiles without the user adding anything.
    #[test]
    fn generated_cargo_toml_has_dev_deps_for_testing() {
        let tmp = TempDir::new().unwrap();
        generate("dev-dep-app", tmp.path()).unwrap();
        let content = fs::read_to_string(tmp.path().join("dev-dep-app/Cargo.toml")).unwrap();
        assert!(
            content.contains("[dev-dependencies]"),
            "generated Cargo.toml must have [dev-dependencies]"
        );
        assert!(
            content.contains("tokio"),
            "generated Cargo.toml must include tokio in dev-dependencies for #[tokio::test]"
        );
    }

    #[test]
    fn cargo_toml_has_project_name() {
        let tmp = TempDir::new().unwrap();
        generate("my-cool-app", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("my-cool-app/Cargo.toml")).unwrap();
        assert!(content.contains(r#"name = "my-cool-app""#));
        assert!(content.contains("autumn-web = "));
    }

    #[test]
    fn cargo_toml_has_autumn_version() {
        let tmp = TempDir::new().unwrap();
        generate("ver-check", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("ver-check/Cargo.toml")).unwrap();
        let expected = format!(r#"autumn-web = "{}""#, env!("CARGO_PKG_VERSION"));
        assert!(content.contains(&expected));
    }

    #[test]
    fn main_rs_has_sample_routes() {
        let tmp = TempDir::new().unwrap();
        generate("route-check", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("route-check/src/main.rs")).unwrap();
        assert!(content.contains(r#"#[get("/")]"#));
        assert!(content.contains(r#"#[get("/hello")]"#));
        assert!(content.contains(r#"#[get("/hello/{name}")]"#));
        assert!(content.contains("#[autumn_web::main]"));
        assert!(content.contains("autumn_web::app()"));
    }

    #[test]
    fn autumn_toml_has_defaults() {
        let tmp = TempDir::new().unwrap();
        generate("cfg-check", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("cfg-check/autumn.toml")).unwrap();
        assert!(content.contains("port = 3000"));
        assert!(content.contains(r#"host = "127.0.0.1""#));
        assert!(content.contains(r#"level = "info""#));
        assert!(content.contains(r#"path = "/health""#));
    }

    #[test]
    fn autumn_toml_has_crate_name_in_db_url() {
        let tmp = TempDir::new().unwrap();
        generate("my-db-app", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("my-db-app/autumn.toml")).unwrap();
        assert!(content.contains("my_db_app"));
    }

    #[test]
    fn gitignore_excludes_target_and_css() {
        let tmp = TempDir::new().unwrap();
        generate("gi-check", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("gi-check/.gitignore")).unwrap();
        assert!(content.contains("/target"));
        assert!(content.contains("static/css/autumn.css"));
        assert!(!content.contains("static/autumn/"));
    }

    #[test]
    fn generated_build_rs_reruns_on_css_input_changes() {
        let tmp = TempDir::new().unwrap();
        generate("css-watch-check", tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("css-watch-check/build.rs")).unwrap();
        assert!(content.contains("cargo:rerun-if-changed=static/css/input.css"));
        assert!(content.contains("cargo:rerun-if-changed=target/autumn/tailwindcss"));
        assert!(content.contains("cargo:rerun-if-env-changed=PATH"));
    }

    #[test]
    fn no_unsubstituted_placeholders() {
        let tmp = TempDir::new().unwrap();
        generate("placeholder-check", tmp.path()).unwrap();

        let p = tmp.path().join("placeholder-check");
        for entry in walkdir(&p) {
            let content = fs::read_to_string(&entry).unwrap();
            assert!(
                !content.contains("{{"),
                "unsubstituted placeholder in {}",
                entry.display()
            );
        }
    }

    #[test]
    fn already_exists_error() {
        let tmp = TempDir::new().unwrap();
        generate("dupe-check", tmp.path()).unwrap();
        let err = generate("dupe-check", tmp.path()).unwrap_err();
        assert!(matches!(err, NewError::AlreadyExists(_)));
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn invalid_name_error() {
        let tmp = TempDir::new().unwrap();
        let err = generate("123bad", tmp.path()).unwrap_err();
        assert!(matches!(err, NewError::InvalidName(_, _)));
    }

    // ── --with-i18n scaffold ─────────────────────────────────────

    #[test]
    fn default_does_not_scaffold_i18n() {
        let tmp = TempDir::new().unwrap();
        generate("plain-app", tmp.path()).unwrap();
        let p = tmp.path().join("plain-app");
        assert!(!p.join("i18n").exists());
        let cargo = fs::read_to_string(p.join("Cargo.toml")).unwrap();
        assert!(!cargo.contains("features = [\"i18n\"]"));
        let toml = fs::read_to_string(p.join("autumn.toml")).unwrap();
        assert!(!toml.contains("[i18n]"));
        let main = fs::read_to_string(p.join("src/main.rs")).unwrap();
        assert!(!main.contains(".i18n_auto()"));
    }

    fn daemon_opts() -> GenerateOptions {
        GenerateOptions {
            with_daemon: true,
            ..GenerateOptions::default()
        }
    }

    #[test]
    fn daemon_starter_omits_db_feature() {
        let tmp = TempDir::new().unwrap();
        generate_with("daemon-app", tmp.path(), daemon_opts()).unwrap();
        let cargo = fs::read_to_string(tmp.path().join("daemon-app/Cargo.toml")).unwrap();
        assert!(
            cargo.contains("default-features = false"),
            "daemon starter must disable default features (drop db): {cargo}"
        );
        assert!(!cargo.contains("\"db\""), "daemon starter must not enable db");
        assert!(
            !cargo.contains("diesel_migrations"),
            "daemon starter must not depend on diesel_migrations"
        );
    }

    #[test]
    fn daemon_starter_main_has_no_migrations() {
        let tmp = TempDir::new().unwrap();
        generate_with("daemon-main-app", tmp.path(), daemon_opts()).unwrap();
        let main = fs::read_to_string(tmp.path().join("daemon-main-app/src/main.rs")).unwrap();
        assert!(!main.contains(".migrations("), "daemon main must not call .migrations()");
        assert!(
            !main.contains("embed_migrations"),
            "daemon main must not embed migrations"
        );
    }

    #[test]
    fn daemon_starter_autumn_toml_documents_zero_db() {
        let tmp = TempDir::new().unwrap();
        generate_with("daemon-cfg-app", tmp.path(), daemon_opts()).unwrap();
        let toml = fs::read_to_string(tmp.path().join("daemon-cfg-app/autumn.toml")).unwrap();
        assert!(toml.contains("autumn serve"));
    }

    #[test]
    fn default_generation_still_has_db() {
        let tmp = TempDir::new().unwrap();
        generate("plain-app", tmp.path()).unwrap();
        let cargo = fs::read_to_string(tmp.path().join("plain-app/Cargo.toml")).unwrap();
        // Default keeps the simple full-default dependency and migrations.
        assert!(cargo.contains(r#"autumn-web = ""#));
        assert!(!cargo.contains("default-features = false"));
        assert!(cargo.contains("diesel_migrations"));
        let main = fs::read_to_string(tmp.path().join("plain-app/src/main.rs")).unwrap();
        assert!(main.contains(".migrations("));
    }

    #[test]
    fn with_i18n_scaffolds_translation_dir_and_stub_file() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "i18n-app",
            tmp.path(),
            GenerateOptions {
                with_i18n: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        let p = tmp.path().join("i18n-app");
        assert!(p.join("i18n").is_dir(), "i18n/ dir not created");
        assert!(
            p.join("i18n/en.ftl").is_file(),
            "i18n/en.ftl stub not created"
        );
        let stub = fs::read_to_string(p.join("i18n/en.ftl")).unwrap();
        assert!(stub.contains("welcome.title"));
        assert!(stub.contains("welcome.greeting"));
    }

    #[test]
    fn with_i18n_enables_feature_flag_in_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "feat-app",
            tmp.path(),
            GenerateOptions {
                with_i18n: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        let cargo = fs::read_to_string(tmp.path().join("feat-app/Cargo.toml")).unwrap();
        assert!(
            cargo.contains(r#"features = ["i18n"]"#),
            "Cargo.toml should enable i18n feature: {cargo}"
        );
    }

    #[test]
    fn with_i18n_adds_block_to_autumn_toml() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "cfg-app",
            tmp.path(),
            GenerateOptions {
                with_i18n: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        let cfg = fs::read_to_string(tmp.path().join("cfg-app/autumn.toml")).unwrap();
        assert!(cfg.contains("[i18n]"));
        assert!(cfg.contains("default_locale = \"en\""));
        assert!(cfg.contains("supported_locales = [\"en\"]"));
    }

    #[test]
    fn with_i18n_calls_i18n_auto_in_main() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "main-app",
            tmp.path(),
            GenerateOptions {
                with_i18n: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        let main = fs::read_to_string(tmp.path().join("main-app/src/main.rs")).unwrap();
        assert!(
            main.contains(".i18n_auto()"),
            "main.rs should call .i18n_auto(): {main}"
        );
    }

    fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(walkdir(&path));
                } else {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext != "enc" {
                        files.push(path);
                    }
                }
            }
        }
        files
    }

    // ── --with-seed tests ──────────────────────────────────────────────────

    #[test]
    fn no_seed_bin_without_flag() {
        let tmp = TempDir::new().unwrap();
        generate("no-seed-app", tmp.path()).unwrap();
        assert!(!tmp.path().join("no-seed-app/src/bin/seed.rs").exists());
    }

    #[test]
    fn generates_seed_bin_when_with_seed() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "seed-app",
            tmp.path(),
            GenerateOptions {
                with_seed: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        assert!(tmp.path().join("seed-app/src/bin/seed.rs").is_file());
    }

    #[test]
    fn with_seed_cargo_toml_has_bin_entry_and_seed_feature() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "seed-cargo",
            tmp.path(),
            GenerateOptions {
                with_seed: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();
        let content = fs::read_to_string(tmp.path().join("seed-cargo/Cargo.toml")).unwrap();
        assert!(
            content.contains("[[bin]]"),
            "Cargo.toml should have [[bin]]"
        );
        assert!(
            content.contains("seed"),
            "Cargo.toml [[bin]] entry should mention 'seed'"
        );
        // The seed feature must be enabled on autumn-web so src/bin/seed.rs
        // can import autumn_web::seed::SeedContext without manual edits.
        assert!(
            content.contains(r#"features = ["seed"]"#),
            "autumn-web dependency should include the seed feature, got:\n{content}"
        );
    }

    #[test]
    fn with_i18n_and_seed_combines_feature_flags() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "combo-app",
            tmp.path(),
            GenerateOptions {
                with_i18n: true,
                with_seed: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();

        let p = tmp.path().join("combo-app");
        let cargo = fs::read_to_string(p.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains(r#"features = ["i18n", "seed"]"#),
            "Cargo.toml should preserve both optional features: {cargo}"
        );
        assert!(p.join("i18n/en.ftl").is_file());
        assert!(p.join("src/bin/seed.rs").is_file());
    }

    #[test]
    fn no_bin_entry_in_cargo_toml_without_flag() {
        let tmp = TempDir::new().unwrap();
        generate("plain-cargo", tmp.path()).unwrap();
        let content = fs::read_to_string(tmp.path().join("plain-cargo/Cargo.toml")).unwrap();
        assert!(
            !content.contains("[[bin]]"),
            "Cargo.toml should not have [[bin]] when --with-seed is off"
        );
    }

    #[test]
    fn with_seed_no_unsubstituted_placeholders() {
        let tmp = TempDir::new().unwrap();
        generate_with(
            "seed-placeholder-check",
            tmp.path(),
            GenerateOptions {
                with_seed: true,
                ..GenerateOptions::default()
            },
        )
        .unwrap();

        let p = tmp.path().join("seed-placeholder-check");
        for entry in walkdir(&p) {
            let content = fs::read_to_string(&entry).unwrap();
            assert!(
                !content.contains("{{"),
                "unsubstituted placeholder in {}",
                entry.display()
            );
        }
    }

    // ── credentials scaffolding tests ─────────────────────────────────────

    #[test]
    fn generates_config_credentials_directory() {
        let tmp = TempDir::new().unwrap();
        generate("cred-app", tmp.path()).unwrap();
        let p = tmp.path().join("cred-app");
        assert!(
            p.join("config/credentials").is_dir(),
            "config/credentials/ directory must be created by autumn new"
        );
    }

    #[test]
    fn generates_development_enc_file() {
        let tmp = TempDir::new().unwrap();
        generate("cred-enc-app", tmp.path()).unwrap();
        let p = tmp.path().join("cred-enc-app");
        assert!(
            p.join("config/credentials/development.toml.enc").is_file(),
            "config/credentials/development.toml.enc must be created by autumn new"
        );
    }

    #[test]
    fn generates_master_key_file() {
        let tmp = TempDir::new().unwrap();
        generate("key-app", tmp.path()).unwrap();
        let p = tmp.path().join("key-app");
        assert!(
            p.join("config/master.key").is_file(),
            "config/master.key must be created by autumn new"
        );
    }

    #[test]
    fn master_key_file_contains_64_hex_chars() {
        let tmp = TempDir::new().unwrap();
        generate("key-hex-app", tmp.path()).unwrap();
        let key = fs::read_to_string(tmp.path().join("key-hex-app/config/master.key")).unwrap();
        let key = key.trim();
        assert_eq!(key.len(), 64, "master.key must contain 64 hex chars");
        assert!(
            key.chars().all(|c| c.is_ascii_hexdigit()),
            "master.key must be valid hex"
        );
    }

    #[test]
    fn gitignore_includes_master_key() {
        let tmp = TempDir::new().unwrap();
        generate("gi-key-app", tmp.path()).unwrap();
        let content = fs::read_to_string(tmp.path().join("gi-key-app/.gitignore")).unwrap();
        assert!(
            content.contains("config/master.key"),
            ".gitignore must exclude config/master.key, got:\n{content}"
        );
    }

    #[test]
    fn gitignore_does_not_exclude_enc_files() {
        let tmp = TempDir::new().unwrap();
        generate("gi-enc-app", tmp.path()).unwrap();
        let content = fs::read_to_string(tmp.path().join("gi-enc-app/.gitignore")).unwrap();
        assert!(
            !content.contains("*.enc"),
            ".gitignore must NOT exclude .enc files (they're safe to commit), got:\n{content}"
        );
    }

    #[test]
    fn development_enc_file_is_decryptable_with_master_key() {
        use autumn_web::credentials::{MasterKey, decrypt};
        let tmp = TempDir::new().unwrap();
        generate("roundtrip-cred-app", tmp.path()).unwrap();
        let p = tmp.path().join("roundtrip-cred-app");
        let key_hex = fs::read_to_string(p.join("config/master.key")).unwrap();
        let key = MasterKey::from_hex_pub(key_hex.trim()).expect("master.key should be valid hex");
        let ct = fs::read(p.join("config/credentials/development.toml.enc")).unwrap();
        let pt = decrypt(&key, &ct).expect("development.toml.enc should decrypt with master.key");
        let s = String::from_utf8(pt).unwrap();
        assert!(
            s.contains("stripe_secret_key") || s.contains('#'),
            "decrypted content should have placeholder comments"
        );
    }

    #[test]
    fn two_new_projects_get_different_master_keys() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        generate("app-a", tmp1.path()).unwrap();
        generate("app-b", tmp2.path()).unwrap();
        let k1 = fs::read_to_string(tmp1.path().join("app-a/config/master.key")).unwrap();
        let k2 = fs::read_to_string(tmp2.path().join("app-b/config/master.key")).unwrap();
        assert_ne!(
            k1.trim(),
            k2.trim(),
            "each project must get a unique master key"
        );
    }

    #[cfg(unix)]
    #[test]
    fn master_key_file_has_secure_permissions() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new().unwrap();
        generate("secure-key-app", tmp.path()).unwrap();
        let p = tmp.path().join("secure-key-app/config/master.key");
        let meta = fs::metadata(&p).unwrap();
        let mode = meta.mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "master.key permissions must be 0o600, got {mode:#o}"
        );
    }
}
