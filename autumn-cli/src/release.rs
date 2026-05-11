//! Production deployment scaffolding for `autumn release init`.
//!
//! Emits a curated set of files (Dockerfile, .dockerignore, config example,
//! and optional target-specific scaffolds) at the project root.

use std::fs;
use std::path::Path;

mod templates {
    pub const DOCKERFILE: &str = include_str!("templates/release/Dockerfile.tmpl");
    pub const DOCKERIGNORE: &str = include_str!("templates/release/.dockerignore.tmpl");
    pub const PRODUCTION_CONFIG: &str =
        include_str!("templates/release/autumn.production.toml.example.tmpl");
    pub const FLY_TOML: &str = include_str!("templates/release/fly.toml.tmpl");
    pub const DOCKER_COMPOSE: &str = include_str!("templates/release/docker-compose.yml.tmpl");
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("'{0}' already exists — run with --force to overwrite")]
    FileExists(String),

    #[error("failed to read Cargo.toml: {0}")]
    CargoToml(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Default,
    Fly,
    DockerCompose,
}

impl std::str::FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fly" => Ok(Self::Fly),
            "docker-compose" => Ok(Self::DockerCompose),
            other => Err(format!(
                "unknown target '{other}'; expected 'fly' or 'docker-compose'"
            )),
        }
    }
}

#[derive(Clone, Copy)]
pub enum ReleaseAction {
    Init { force: bool, target: Target },
}

pub fn run(action: ReleaseAction) {
    eprintln!("🍂 autumn release\n");

    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine current directory: {e}");
        std::process::exit(1);
    });

    match action {
        ReleaseAction::Init { force, target } => {
            let project_name = read_project_name(&cwd).unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            });

            match init(&cwd, &project_name, force, target) {
                Ok(files) => {
                    for f in &files {
                        println!("  Created {f}");
                    }

                    // Smoke gate: verify the generated production config does
                    // not contain a committed signing secret literal.
                    let config_path = cwd.join("autumn.production.toml.example");
                    if let Ok(content) = std::fs::read_to_string(&config_path)
                        && let Err(e) = check_production_config_signing_secret(&content)
                    {
                        eprintln!("Warning: smoke gate failed for generated config: {e}");
                    }

                    println!();
                    println!("Next steps:");
                    println!(
                        "  1. Generate and set your signing secret (REQUIRED before production boot):"
                    );
                    println!(
                        "       export AUTUMN_SECURITY__SIGNING_SECRET=\"$(openssl rand -hex 32)\""
                    );
                    println!("     Smoke-gate check — the app must refuse to start without it:");
                    println!("       AUTUMN_ENV=prod docker run --rm \\");
                    println!("         -e AUTUMN_DATABASE__PRIMARY_URL=... \\");
                    println!("         {project_name} 2>&1 | grep -i 'signing secret'");
                    println!("     And must start with it:");
                    println!("       AUTUMN_ENV=prod docker run --rm \\");
                    println!("         -e AUTUMN_DATABASE__PRIMARY_URL=... \\");
                    println!(
                        "         -e AUTUMN_SECURITY__SIGNING_SECRET=\"$AUTUMN_SECURITY__SIGNING_SECRET\" \\"
                    );
                    println!("         {project_name}");
                    println!();
                    println!("  2. Build, migrate the primary once, then run web replicas:");
                    println!("       docker build -t {project_name} .");
                    println!("       AUTUMN_DATABASE__PRIMARY_URL=... autumn migrate");
                    println!(
                        "       docker run --rm -p 3000:3000 -e AUTUMN_DATABASE__PRIMARY_URL=... \\"
                    );
                    println!(
                        "         -e AUTUMN_SECURITY__SIGNING_SECRET=\"$AUTUMN_SECURITY__SIGNING_SECRET\" \\"
                    );
                    println!("         {project_name}");
                    println!();
                    println!(
                        "  See docs/guide/deployment.md and docs/guide/signing-secrets.md for the full walkthrough."
                    );
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Validate a generated production config file for signing-secret compliance.
///
/// Returns `Ok(())` when the config file correctly documents the signing
/// secret via an environment variable reference (not a committed literal value).
/// Returns `Err` with a human-readable explanation when the file contains a
/// committed secret literal.
///
/// Used by the release checklist smoke gate to verify that generated
/// deployment files obey the "never commit secrets" rule.
///
/// # Errors
///
/// Returns a string error message when a raw signing secret literal is found
/// in a non-comment line of `content`.
pub fn check_production_config_signing_secret(content: &str) -> Result<(), String> {
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        // A non-comment line containing a `secret =` assignment with a non-empty
        // RHS is a committed secret literal — a critical misconfiguration.
        if let Some(rest) = trimmed.strip_prefix("secret") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() && value != "[]" {
                    return Err(format!(
                        "line {}: production config contains a committed signing secret literal \
                         at `secret = ...`; use AUTUMN_SECURITY__SIGNING_SECRET env var instead",
                        line_num + 1,
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn read_project_name(dir: &Path) -> Result<String, ReleaseError> {
    let path = dir.join("Cargo.toml");
    let content = fs::read_to_string(&path)
        .map_err(|e| ReleaseError::CargoToml(format!("{}: {e}", path.display())))?;

    // Check for workspace root before parsing; workspace-only Cargo.toml files
    // may not parse cleanly as a member manifest.
    if content.contains("[workspace]") && !content.contains("[package]") {
        return Err(ReleaseError::CargoToml(
            "found [workspace] but no [package] — run this command from a member crate directory, not the workspace root".into(),
        ));
    }

    let parsed: toml::Value = content
        .parse()
        .map_err(|e| ReleaseError::CargoToml(format!("parse error: {e}")))?;
    parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ReleaseError::CargoToml("missing [package] name".into()))
}

/// Emit release scaffolding files into `dir` for the given `project_name`.
///
/// Returns the list of file names written. Returns [`ReleaseError::FileExists`]
/// if any output file already exists and `force` is `false`.
pub fn init(
    dir: &Path,
    project_name: &str,
    force: bool,
    target: Target,
) -> Result<Vec<String>, ReleaseError> {
    let files = planned_files(target);

    if !force {
        for (name, _) in &files {
            if dir.join(name).exists() {
                return Err(ReleaseError::FileExists(name.to_string()));
            }
        }
    }

    let mut created = Vec::new();
    for (name, template) in files {
        fs::write(dir.join(name), render(template, project_name))?;
        created.push(name.to_string());
    }
    Ok(created)
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn render(template: &str, project_name: &str) -> String {
    template
        .replace("{{project_name}}", project_name)
        .replace("{{autumn_cli_version}}", env!("CARGO_PKG_VERSION"))
}

fn planned_files(target: Target) -> Vec<(&'static str, &'static str)> {
    let mut files: Vec<(&'static str, &'static str)> = vec![
        ("Dockerfile", templates::DOCKERFILE),
        (".dockerignore", templates::DOCKERIGNORE),
        (
            "autumn.production.toml.example",
            templates::PRODUCTION_CONFIG,
        ),
    ];
    match target {
        Target::Fly => files.push(("fly.toml", templates::FLY_TOML)),
        Target::DockerCompose => files.push(("docker-compose.yml", templates::DOCKER_COMPOSE)),
        Target::Default => {}
    }
    files
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_project(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
        dir
    }

    // ── default target ────────────────────────────────────────────────────────

    #[test]
    fn init_creates_dockerfile() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(dir.join("Dockerfile").is_file(), "Dockerfile not created");
    }

    #[test]
    fn init_creates_dockerignore() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(
            dir.join(".dockerignore").is_file(),
            ".dockerignore not created"
        );
    }

    #[test]
    fn init_creates_production_config_example() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(
            dir.join("autumn.production.toml.example").is_file(),
            "autumn.production.toml.example not created"
        );
    }

    #[test]
    fn init_returns_list_of_created_files() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        let files = init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(files.contains(&"Dockerfile".to_string()));
        assert!(files.contains(&".dockerignore".to_string()));
        assert!(files.contains(&"autumn.production.toml.example".to_string()));
    }

    // ── Dockerfile content ────────────────────────────────────────────────────

    #[test]
    fn dockerfile_has_cargo_chef_stages() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("cargo-chef"),
            "Dockerfile must use cargo-chef for dependency caching"
        );
        assert!(
            content.contains("cargo chef prepare"),
            "Dockerfile must run 'cargo chef prepare'"
        );
        assert!(
            content.contains("cargo chef cook"),
            "Dockerfile must run 'cargo chef cook'"
        );
    }

    #[test]
    fn dockerfile_has_three_stages() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        let from_count = content.lines().filter(|l| l.starts_with("FROM ")).count();
        assert!(
            from_count >= 3,
            "Dockerfile must have at least 3 FROM stages (chef, planner, builder, runtime); got {from_count}"
        );
    }

    #[test]
    fn dockerfile_copies_production_config_as_runtime_autumn_toml() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("autumn.production.toml.example"),
            "Dockerfile must COPY autumn.production.toml.example into the runtime image so \
             the container binds to 0.0.0.0 (not the dev 127.0.0.1) without manual edits"
        );
    }

    #[test]
    fn dockerfile_runtime_uses_debian_slim() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("debian:bookworm-slim"),
            "runtime stage must use debian:bookworm-slim"
        );
    }

    #[test]
    fn dockerfile_runtime_installs_libpq_and_tini() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("libpq"),
            "runtime must install libpq for Diesel"
        );
        assert!(
            content.contains("tini"),
            "runtime must install tini as init process"
        );
        assert!(
            content.contains("ca-certificates"),
            "runtime must install ca-certificates"
        );
    }

    #[test]
    fn dockerfile_copies_static_assets() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("static"),
            "Dockerfile must COPY static/ assets into runtime image"
        );
    }

    #[test]
    fn dockerfile_copies_migrations() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("migrations"),
            "Dockerfile must COPY migrations into runtime image"
        );
    }

    #[test]
    fn dockerfile_defers_migrations_to_one_shot_primary_job() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            !content.contains("auto_migrate_in_production = true"),
            "Dockerfile must not enable startup migrations for every web replica"
        );
        assert!(
            content.contains("autumn migrate"),
            "Dockerfile must document the explicit primary-role migration job"
        );
    }

    #[test]
    fn dockerfile_installs_autumn_cli_for_migration_jobs() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("cargo install")
                && content.contains("autumn-cli")
                && content.contains("/usr/local/bin/autumn"),
            "Dockerfile must include the autumn CLI used by one-shot migration jobs"
        );
    }

    #[test]
    fn dockerfile_has_healthcheck() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("HEALTHCHECK"),
            "Dockerfile must have a HEALTHCHECK directive"
        );
        assert!(
            content.contains("/health"),
            "HEALTHCHECK must probe the /health actuator endpoint"
        );
    }

    #[test]
    fn dockerfile_exposes_port_3000() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("EXPOSE 3000"),
            "Dockerfile must EXPOSE 3000"
        );
    }

    #[test]
    fn dockerfile_substitutes_project_name() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-blog");
        init(&dir, "my-blog", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            content.contains("my-blog"),
            "Dockerfile must contain the substituted project name"
        );
        assert!(
            !content.contains("{{project_name}}"),
            "Dockerfile must not contain unsubstituted {{{{project_name}}}}"
        );
    }

    // ── .dockerignore content ─────────────────────────────────────────────────

    #[test]
    fn dockerignore_excludes_target() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join(".dockerignore")).unwrap();
        assert!(
            content.contains("target"),
            ".dockerignore must exclude target/"
        );
    }

    #[test]
    fn dockerignore_excludes_git() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join(".dockerignore")).unwrap();
        assert!(content.contains(".git"), ".dockerignore must exclude .git");
    }

    #[test]
    fn dockerignore_excludes_node_modules() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join(".dockerignore")).unwrap();
        assert!(
            content.contains("node_modules"),
            ".dockerignore must exclude node_modules"
        );
    }

    #[test]
    fn dockerignore_excludes_dist() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join(".dockerignore")).unwrap();
        assert!(content.contains("dist"), ".dockerignore must exclude dist/");
    }

    // ── signing-secret smoke gate ─────────────────────────────────────────────

    #[test]
    fn production_config_template_documents_signing_secret_env_var() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("AUTUMN_SECURITY__SIGNING_SECRET"),
            "production config template must document the signing-secret env var"
        );
    }

    #[test]
    fn production_config_template_documents_openssl_rand_command() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("openssl rand -hex 32"),
            "production config template must show the secret generation command"
        );
    }

    #[test]
    fn production_config_template_mentions_signing_secrets_guide() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("signing-secrets.md"),
            "production config template must link to the signing-secrets guide"
        );
    }

    #[test]
    fn smoke_gate_passes_for_valid_config() {
        let content = r#"
# This is a comment with secret = "ignored"
[server]
port = 3000
"#;
        assert!(check_production_config_signing_secret(content).is_ok());
    }

    #[test]
    fn smoke_gate_fails_when_secret_literal_committed() {
        let content = r#"
[security.signing_secret]
secret = "my-actual-secret-value-here"
"#;
        let err = check_production_config_signing_secret(content).unwrap_err();
        assert!(err.contains("committed signing secret literal"));
    }

    #[test]
    fn smoke_gate_ignores_commented_secret_lines() {
        // Comments are allowed to mention the key name for documentation.
        let content = r#"
# secret = "example-value-for-docs"
# Set AUTUMN_SECURITY__SIGNING_SECRET instead
"#;
        assert!(check_production_config_signing_secret(content).is_ok());
    }

    #[test]
    fn smoke_gate_passes_for_empty_previous_secrets() {
        let content = "
[security.signing_secret]
previous_secrets = []
";
        assert!(check_production_config_signing_secret(content).is_ok());
    }

    // ── production config content ─────────────────────────────────────────────

    #[test]
    fn production_config_has_placeholder_db_url_not_real_credentials() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        // Must have a DB URL entry
        assert!(
            content.contains("DATABASE_URL") || content.contains("url"),
            "production config must document the database URL setting"
        );
        // Must not contain real credentials (no 'password' in a non-commented line)
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            assert!(
                !trimmed.to_lowercase().contains("secret"),
                "production config must not contain real secrets"
            );
        }
    }

    #[test]
    fn production_config_has_placeholder_for_project_name() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-blog");
        init(&dir, "my-blog", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("my-blog"),
            "production config must substitute project name"
        );
        assert!(
            !content.contains("{{project_name}}"),
            "production config must not contain unsubstituted placeholders"
        );
    }

    #[test]
    fn production_config_documents_port() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("port"),
            "production config must document the port setting"
        );
    }

    // ── --force flag ──────────────────────────────────────────────────────────

    #[test]
    fn init_without_force_errors_if_dockerfile_exists() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        fs::write(dir.join("Dockerfile"), "existing content").unwrap();
        let err = init(&dir, "my-app", false, Target::Default).unwrap_err();
        assert!(
            matches!(err, ReleaseError::FileExists(_)),
            "expected FileExists, got: {err}"
        );
        assert!(
            err.to_string().contains("Dockerfile"),
            "error message must name the conflicting file"
        );
    }

    #[test]
    fn init_without_force_errors_if_any_file_exists() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        fs::write(dir.join(".dockerignore"), "existing").unwrap();
        let err = init(&dir, "my-app", false, Target::Default).unwrap_err();
        assert!(matches!(err, ReleaseError::FileExists(_)));
    }

    #[test]
    fn init_with_force_overwrites_existing_files() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        fs::write(dir.join("Dockerfile"), "old content").unwrap();
        init(&dir, "my-app", true, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert_ne!(
            content, "old content",
            "Dockerfile must be overwritten with --force"
        );
    }

    // ── --target=fly ──────────────────────────────────────────────────────────

    #[test]
    fn fly_target_creates_fly_toml() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Fly).unwrap();
        assert!(
            dir.join("fly.toml").is_file(),
            "fly.toml must be created for --target=fly"
        );
    }

    #[test]
    fn fly_toml_references_dockerfile() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Fly).unwrap();
        let content = fs::read_to_string(dir.join("fly.toml")).unwrap();
        assert!(
            content.contains("Dockerfile"),
            "fly.toml must reference the Dockerfile"
        );
    }

    #[test]
    fn fly_toml_has_app_name() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-blog");
        init(&dir, "my-blog", false, Target::Fly).unwrap();
        let content = fs::read_to_string(dir.join("fly.toml")).unwrap();
        assert!(
            content.contains("my-blog"),
            "fly.toml must contain the app name"
        );
        assert!(
            !content.contains("{{project_name}}"),
            "fly.toml must not contain unsubstituted placeholders"
        );
    }

    #[test]
    fn fly_toml_has_healthcheck() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Fly).unwrap();
        let content = fs::read_to_string(dir.join("fly.toml")).unwrap();
        assert!(
            content.contains("/health"),
            "fly.toml must wire the /health endpoint as the healthcheck"
        );
    }

    #[test]
    fn default_target_does_not_create_fly_toml() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(
            !dir.join("fly.toml").exists(),
            "fly.toml must NOT be created for the default target"
        );
    }

    // ── --target=docker-compose ───────────────────────────────────────────────

    #[test]
    fn docker_compose_target_creates_docker_compose_yml() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::DockerCompose).unwrap();
        assert!(
            dir.join("docker-compose.yml").is_file(),
            "docker-compose.yml must be created for --target=docker-compose"
        );
    }

    #[test]
    fn docker_compose_has_app_service() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::DockerCompose).unwrap();
        let content = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();
        assert!(
            content.contains("app:"),
            "docker-compose.yml must have an 'app' service"
        );
    }

    #[test]
    fn docker_compose_has_postgres_service() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::DockerCompose).unwrap();
        let content = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();
        assert!(
            content.contains("postgres") || content.contains("db:"),
            "docker-compose.yml must have a Postgres service"
        );
    }

    #[test]
    fn docker_compose_app_depends_on_db() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::DockerCompose).unwrap();
        let content = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();
        assert!(
            content.contains("depends_on"),
            "docker-compose.yml app service must depend_on the db"
        );
    }

    #[test]
    fn docker_compose_runs_one_shot_migration_before_app() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::DockerCompose).unwrap();
        let content = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();

        assert!(
            content.contains("migrate:"),
            "docker-compose.yml must include a one-shot migration service"
        );
        assert!(
            content.contains("autumn migrate"),
            "migration service must run autumn migrate"
        );
        assert!(
            content.contains("condition: service_completed_successfully"),
            "app service must wait for the migration job to complete successfully"
        );
    }

    #[test]
    fn docker_compose_substitutes_project_name() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-blog");
        init(&dir, "my-blog", false, Target::DockerCompose).unwrap();
        let content = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();
        assert!(
            content.contains("my-blog"),
            "docker-compose.yml must substitute project name"
        );
        assert!(
            !content.contains("{{project_name}}"),
            "docker-compose.yml must not contain unsubstituted placeholders"
        );
    }

    #[test]
    fn default_target_does_not_create_docker_compose() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        assert!(
            !dir.join("docker-compose.yml").exists(),
            "docker-compose.yml must NOT be created for the default target"
        );
    }

    // ── workspace root error ──────────────────────────────────────────────────

    #[test]
    fn workspace_root_gives_actionable_hint() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"my-app\"]\n",
        )
        .unwrap();
        let err = read_project_name(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("workspace"),
            "error must mention workspace: {msg}"
        );
        assert!(
            msg.contains("member"),
            "error must hint to run from a member directory: {msg}"
        );
    }

    // ── auto-migration config ─────────────────────────────────────────────────

    #[test]
    fn production_config_disables_startup_migrations_by_default() {
        let tmp = TempDir::new().unwrap();
        let dir = make_project(&tmp, "my-app");
        init(&dir, "my-app", false, Target::Default).unwrap();
        let content = fs::read_to_string(dir.join("autumn.production.toml.example")).unwrap();
        assert!(
            content.contains("auto_migrate_in_production = false"),
            "production config must leave web replicas out of migration ownership"
        );
        assert!(
            content.contains("primary_url"),
            "production config must name the primary/write database role"
        );
        assert!(
            content.contains("autumn migrate"),
            "production config must point operators at the one-shot migration command"
        );
    }

    // ── target parsing ────────────────────────────────────────────────────────

    #[test]
    fn parse_target_fly() {
        assert_eq!("fly".parse::<Target>().unwrap(), Target::Fly);
    }

    #[test]
    fn parse_target_docker_compose() {
        assert_eq!(
            "docker-compose".parse::<Target>().unwrap(),
            Target::DockerCompose
        );
    }

    #[test]
    fn parse_target_unknown_is_error() {
        assert!("kubernetes".parse::<Target>().is_err());
    }
}
