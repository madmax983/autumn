//! Project scaffolding for `autumn new <name>`.
//!
//! Generates a complete Autumn project directory from embedded templates.

use std::fs;
use std::path::Path;

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

/// Entry point called from `main.rs` and delegates to [`generate`].
pub fn run(name: &str, with_seed: bool) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine current directory: {e}");
        std::process::exit(1);
    });
    if let Err(e) = generate(name, &cwd, with_seed) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Generate a new Autumn project under `parent_dir/name`.
///
/// When `with_seed` is `true`, scaffolds `src/bin/seed.rs` and adds the
/// `[[bin]]` entry to `Cargo.toml`.
pub fn generate(name: &str, parent_dir: &Path, with_seed: bool) -> Result<(), NewError> {
    validate_name(name)?;

    let project_dir = parent_dir.join(name);
    if project_dir.exists() {
        return Err(NewError::AlreadyExists(name.to_owned()));
    }

    let crate_name = name.replace('-', "_");
    let autumn_version = env!("CARGO_PKG_VERSION");

    fs::create_dir_all(project_dir.join("src"))?;
    fs::create_dir_all(project_dir.join("static/css"))?;
    fs::create_dir_all(project_dir.join("migrations"))?;

    let render = |template: &str| -> String {
        template
            .replace("{{project_name}}", name)
            .replace("{{crate_name}}", &crate_name)
            .replace("{{autumn_version}}", autumn_version)
    };

    // Build Cargo.toml: base + optional seed [[bin]] entry.
    // When --with-seed is on, upgrade the plain `autumn-web = "x"` dependency
    // to a table form that enables the `seed` feature so `src/bin/seed.rs`
    // can import `autumn_web::seed::SeedContext` without manual edits.
    let cargo_toml = if with_seed {
        let plain_dep = format!(r#"autumn-web = "{autumn_version}""#);
        let seed_dep =
            format!(r#"autumn-web = {{ version = "{autumn_version}", features = ["seed"] }}"#);
        let base = render(templates::CARGO_TOML).replace(&plain_dep, &seed_dep);
        format!("{base}\n{}", render(templates::SEED_CARGO_TOML))
    } else {
        render(templates::CARGO_TOML)
    };
    fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

    fs::write(project_dir.join("src/main.rs"), render(templates::MAIN_RS))?;
    fs::write(
        project_dir.join("autumn.toml"),
        render(templates::AUTUMN_TOML),
    )?;
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

    if with_seed {
        fs::create_dir_all(project_dir.join("src/bin"))?;
        fs::write(
            project_dir.join("src/bin/seed.rs"),
            render(templates::SEED_RS),
        )?;
    }

    println!("  Created {name}/");
    println!("  Created {name}/Cargo.toml");
    println!("  Created {name}/autumn.toml");
    println!("  Created {name}/Dockerfile");
    println!("  Created {name}/.dockerignore");
    println!("  Created {name}/build.rs");
    println!("  Created {name}/src/main.rs");
    if with_seed {
        println!("  Created {name}/src/bin/seed.rs");
    }
    println!("  Created {name}/static/css/input.css");
    println!("  Created {name}/tailwind.config.js");
    println!("  Created {name}/.gitignore");
    println!("  Created {name}/migrations/");
    println!();
    println!("Get started:");
    println!("  cd {name}");
    println!("  cargo run");
    println!();
    println!("Your app will be available at http://localhost:3000");
    if with_seed {
        println!();
        println!("Seed your database:");
        println!("  autumn migrate && autumn seed");
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
        generate("test-app", tmp.path(), false).unwrap();

        let p = tmp.path().join("test-app");
        assert!(p.join("Cargo.toml").is_file());
        assert!(p.join("src/main.rs").is_file());
        assert!(p.join("autumn.toml").is_file());
        assert!(p.join("Dockerfile").is_file());
        assert!(p.join(".dockerignore").is_file());
        assert!(p.join("build.rs").is_file());
        assert!(p.join(".gitignore").is_file());
        assert!(p.join("static/css/input.css").is_file());
        assert!(p.join("tailwind.config.js").is_file());
        assert!(p.join("migrations/.gitkeep").is_file());
        assert!(!p.join("src/lib.rs").exists());
        assert!(!p.join("src/client.rs").exists());
    }

    #[test]
    fn cargo_toml_has_project_name() {
        let tmp = TempDir::new().unwrap();
        generate("my-cool-app", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("my-cool-app/Cargo.toml")).unwrap();
        assert!(content.contains(r#"name = "my-cool-app""#));
        assert!(content.contains("autumn-web = "));
    }

    #[test]
    fn cargo_toml_has_autumn_version() {
        let tmp = TempDir::new().unwrap();
        generate("ver-check", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("ver-check/Cargo.toml")).unwrap();
        let expected = format!(r#"autumn-web = "{}""#, env!("CARGO_PKG_VERSION"));
        assert!(content.contains(&expected));
    }

    #[test]
    fn main_rs_has_sample_routes() {
        let tmp = TempDir::new().unwrap();
        generate("route-check", tmp.path(), false).unwrap();

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
        generate("cfg-check", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("cfg-check/autumn.toml")).unwrap();
        assert!(content.contains("port = 3000"));
        assert!(content.contains(r#"host = "127.0.0.1""#));
        assert!(content.contains(r#"level = "info""#));
        assert!(content.contains(r#"path = "/health""#));
    }

    #[test]
    fn autumn_toml_has_crate_name_in_db_url() {
        let tmp = TempDir::new().unwrap();
        generate("my-db-app", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("my-db-app/autumn.toml")).unwrap();
        assert!(content.contains("my_db_app"));
    }

    #[test]
    fn gitignore_excludes_target_and_css() {
        let tmp = TempDir::new().unwrap();
        generate("gi-check", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("gi-check/.gitignore")).unwrap();
        assert!(content.contains("/target"));
        assert!(content.contains("static/css/autumn.css"));
        assert!(!content.contains("static/autumn/"));
    }

    #[test]
    fn generated_build_rs_reruns_on_css_input_changes() {
        let tmp = TempDir::new().unwrap();
        generate("css-watch-check", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("css-watch-check/build.rs")).unwrap();
        assert!(content.contains("cargo:rerun-if-changed=static/css/input.css"));
        assert!(content.contains("cargo:rerun-if-changed=target/autumn/tailwindcss"));
        assert!(content.contains("cargo:rerun-if-env-changed=PATH"));
    }

    #[test]
    fn no_unsubstituted_placeholders() {
        let tmp = TempDir::new().unwrap();
        generate("placeholder-check", tmp.path(), false).unwrap();

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
        generate("dupe-check", tmp.path(), false).unwrap();
        let err = generate("dupe-check", tmp.path(), false).unwrap_err();
        assert!(matches!(err, NewError::AlreadyExists(_)));
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn invalid_name_error() {
        let tmp = TempDir::new().unwrap();
        let err = generate("123bad", tmp.path(), false).unwrap_err();
        assert!(matches!(err, NewError::InvalidName(_, _)));
    }

    fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(walkdir(&path));
                } else {
                    files.push(path);
                }
            }
        }
        files
    }

    // ── --with-seed tests ──────────────────────────────────────────────────

    #[test]
    fn no_seed_bin_without_flag() {
        let tmp = TempDir::new().unwrap();
        generate("no-seed-app", tmp.path(), false).unwrap();
        assert!(!tmp.path().join("no-seed-app/src/bin/seed.rs").exists());
    }

    #[test]
    fn generates_seed_bin_when_with_seed() {
        let tmp = TempDir::new().unwrap();
        generate("seed-app", tmp.path(), true).unwrap();
        assert!(tmp.path().join("seed-app/src/bin/seed.rs").is_file());
    }

    #[test]
    fn with_seed_cargo_toml_has_bin_entry_and_seed_feature() {
        let tmp = TempDir::new().unwrap();
        generate("seed-cargo", tmp.path(), true).unwrap();
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
    fn no_bin_entry_in_cargo_toml_without_flag() {
        let tmp = TempDir::new().unwrap();
        generate("plain-cargo", tmp.path(), false).unwrap();
        let content = fs::read_to_string(tmp.path().join("plain-cargo/Cargo.toml")).unwrap();
        assert!(
            !content.contains("[[bin]]"),
            "Cargo.toml should not have [[bin]] when --with-seed is off"
        );
    }

    #[test]
    fn with_seed_no_unsubstituted_placeholders() {
        let tmp = TempDir::new().unwrap();
        generate("seed-placeholder-check", tmp.path(), true).unwrap();

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
}
