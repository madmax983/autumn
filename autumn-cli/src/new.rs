//! Project scaffolding for `autumn new <name>`.
//!
//! Generates a complete Autumn project directory from embedded templates.

use std::fs;
use std::path::Path;

mod templates {
    pub const CARGO_TOML: &str = include_str!("templates/Cargo.toml.tmpl");
    pub const MAIN_RS: &str = include_str!("templates/main.rs.tmpl");
    pub const LIB_RS: &str = include_str!("templates/lib.rs.tmpl");
    pub const CLIENT_RS: &str = include_str!("templates/client.rs.tmpl");
    pub const AUTUMN_TOML: &str = include_str!("templates/autumn.toml.tmpl");
    pub const BUILD_RS: &str = include_str!("templates/build.rs.tmpl");
    pub const INPUT_CSS: &str = include_str!("templates/input.css.tmpl");
    pub const TAILWIND_CONFIG: &str = include_str!("templates/tailwind.config.js.tmpl");
    pub const GITIGNORE: &str = include_str!("templates/gitignore.tmpl");
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

/// Entry point called from `main.rs` — resolves CWD and delegates.
pub fn run(name: &str, wasm: bool) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine current directory: {e}");
        std::process::exit(1);
    });
    if let Err(e) = generate(name, &cwd, wasm) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Generate a new Autumn project under `parent_dir/name`.
pub fn generate(name: &str, parent_dir: &Path, wasm: bool) -> Result<(), NewError> {
    validate_name(name)?;

    let project_dir = parent_dir.join(name);
    if project_dir.exists() {
        return Err(NewError::AlreadyExists(name.to_owned()));
    }

    let crate_name = name.replace('-', "_");
    let autumn_version = env!("CARGO_PKG_VERSION");

    // Create directory structure
    fs::create_dir_all(project_dir.join("src"))?;
    fs::create_dir_all(project_dir.join("static/css"))?;
    fs::create_dir_all(project_dir.join("migrations"))?;

    // Render templates with substitution
    let render = |template: &str| -> String {
        let wasm_deps = if wasm {
            format!(
                "autumn-wasm = \"{autumn_version}\"\nserde = {{ version = \"1\", features = [\"derive\"] }}"
            )
        } else {
            String::new()
        };

        template
            .replace("{{project_name}}", name)
            .replace("{{crate_name}}", &crate_name)
            .replace("{{autumn_version}}", autumn_version)
            .replace("{{wasm_deps}}", &wasm_deps)
            .replace(
                "{{cdylib}}",
                if wasm {
                    "crate-type = [\"rlib\", \"cdylib\"]"
                } else {
                    ""
                },
            )
    };

    fs::write(
        project_dir.join("Cargo.toml"),
        render(templates::CARGO_TOML),
    )?;
    fs::write(project_dir.join("src/main.rs"), render(templates::MAIN_RS))?;
    if wasm {
        fs::write(project_dir.join("src/lib.rs"), render(templates::LIB_RS))?;
        fs::write(
            project_dir.join("src/client.rs"),
            render(templates::CLIENT_RS),
        )?;
    }
    fs::write(
        project_dir.join("autumn.toml"),
        render(templates::AUTUMN_TOML),
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

    println!("  Created {name}/");
    println!("  Created {name}/Cargo.toml");
    println!("  Created {name}/autumn.toml");
    println!("  Created {name}/build.rs");
    println!("  Created {name}/src/main.rs");
    if wasm {
        println!("  Created {name}/src/lib.rs");
        println!("  Created {name}/src/client.rs");
    }
    println!("  Created {name}/static/css/input.css");
    println!("  Created {name}/tailwind.config.js");
    println!("  Created {name}/.gitignore");
    println!("  Created {name}/migrations/");
    println!();
    println!("Get started:");
    println!("  cd {name}");
    println!("  cargo run");
    if wasm {
        println!("  rustup target add wasm32-unknown-unknown");
    }
    println!();
    println!("Your app will be available at http://localhost:3000");

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

    // ── Name validation ──────────────────────────────────────────

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

    // ── Project generation ───────────────────────────────────────

    #[test]
    fn generates_all_expected_files() {
        let tmp = TempDir::new().unwrap();
        generate("test-app", tmp.path(), false).unwrap();

        let p = tmp.path().join("test-app");
        assert!(p.join("Cargo.toml").is_file());
        assert!(p.join("src/main.rs").is_file());
        assert!(p.join("autumn.toml").is_file());
        assert!(p.join("build.rs").is_file());
        assert!(p.join(".gitignore").is_file());
        assert!(p.join("static/css/input.css").is_file());
        assert!(p.join("tailwind.config.js").is_file());
        assert!(p.join("migrations/.gitkeep").is_file());
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
        // Hyphens should become underscores in the database URL
        assert!(content.contains("my_db_app"));
    }

    #[test]
    fn gitignore_excludes_target_and_css() {
        let tmp = TempDir::new().unwrap();
        generate("gi-check", tmp.path(), false).unwrap();

        let content = fs::read_to_string(tmp.path().join("gi-check/.gitignore")).unwrap();
        assert!(content.contains("/target"));
        assert!(content.contains("static/css/autumn.css"));
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

    // ── Error cases ──────────────────────────────────────────────

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

    #[test]
    fn wasm_scaffold_adds_client_files() {
        let tmp = TempDir::new().unwrap();
        generate("wasm-app", tmp.path(), true).unwrap();
        let p = tmp.path().join("wasm-app");
        assert!(p.join("src/lib.rs").is_file());
        assert!(p.join("src/client.rs").is_file());
        let cargo = fs::read_to_string(p.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("autumn-wasm"));
    }

    // ── Helpers ──────────────────────────────────────────────────

    /// Recursively collect all files (not directories) under a path.
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
}
