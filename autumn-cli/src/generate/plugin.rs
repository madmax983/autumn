//! autumn generate plugin — plan the generation of a conformant plugin crate.

use std::fs;
use std::path::Path;

use super::emit::Plan;
use super::{Flags, GenerateError};

fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for c in s.chars() {
        if c == '-' || c == '_' {
            capitalize = true;
        } else if capitalize {
            result.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    result
}

#[allow(clippy::too_many_lines)]
pub fn plan_plugin(
    project_root: &Path,
    name: &str,
    path: Option<&str>,
    flags: Flags,
) -> Result<Plan, GenerateError> {
    let target_dir = path.map_or_else(
        || project_root.join(format!("autumn-{name}-plugin")),
        |p| project_root.join(p),
    );

    if target_dir.exists() && !flags.force {
        if target_dir.is_dir() {
            let mut entries = fs::read_dir(&target_dir)?;
            if entries.next().is_some() {
                return Err(GenerateError::Config(format!(
                    "target directory '{}' already exists and is not empty. Use --force to override.",
                    target_dir.display().to_string().replace('\\', "/")
                )));
            }
        } else {
            return Err(GenerateError::Config(format!(
                "target directory '{}' already exists and is not empty. Use --force to override.",
                target_dir.display().to_string().replace('\\', "/")
            )));
        }
    }

    let plan_root = target_dir.parent().unwrap_or(project_root);
    let mut plan = Plan::new(plan_root);

    let cargo_version = env!("CARGO_PKG_VERSION");
    let version_parts: Vec<&str> = cargo_version.split('.').collect();
    let major_minor = if version_parts.len() >= 2 {
        format!("{}.{}", version_parts[0], version_parts[1])
    } else {
        cargo_version.to_string()
    };

    let cargo_toml_content = format!(
        r#"[package]
name = "autumn-{name}-plugin"
version = "0.1.0"
edition = "2024"

[dependencies]
autumn-web = {{ version = "{major_minor}" }}
serde = {{ version = "1", features = ["derive"] }}
"#
    );

    let struct_name = format!("{}Plugin", to_pascal_case(name));
    let lib_rs_content = format!(
        r#"//! autumn-{name}-plugin

use std::borrow::Cow;

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

pub struct {struct_name};

impl {struct_name} {{
    #[must_use]
    pub fn new() -> Self {{
        Self
    }}
}}

impl Default for {struct_name} {{
    fn default() -> Self {{
        Self::new()
    }}
}}

impl Plugin for {struct_name} {{
    fn name(&self) -> Cow<'static, str> {{
        Cow::Borrowed("autumn-{name}-plugin")
    }}

    fn build(self, app: AppBuilder) -> AppBuilder {{
        // Wire a commented example route contribution:
        // app.routes(autumn_web::routes![index])
        app
    }}
}}

// Commented index route function:
// #[autumn_web::get("/index")]
// async fn index() -> &'static str {{
//     "Hello from plugin!"
// }}
"#
    );

    let name_snake = name.replace('-', "_");
    let readme_content = format!(
        r#"# autumn-{name}-plugin

An Autumn plugin for {name}.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
autumn-{name}-plugin = {{ version = "0.1.0" }}
```

## Setup

Register the plugin with your Autumn application in `src/main.rs`:

```rust
use autumn_{name_snake}_plugin::{struct_name};

#[autumn_web::main]
async fn main() {{
    autumn_web::app()
        .plugin({struct_name}::new())
        .run()
        .await;
}}
```
"#
    );

    let conformance_rs_content = format!(
        r#"#[cfg(test)]
mod conformance_tests {{
    use autumn_web::plugin_conformance::{{ConformanceConfig, run_conformance}};
    use autumn_web::route_listing::{{RouteInfo, RouteSource}};

    #[test]
    fn plugin_passes_conformance() {{
        // Simulate the routes your plugin contributes
        let routes = vec![
            RouteInfo {{
                method: "GET".to_owned(),
                path: "/autumn-{name}-plugin".to_owned(),
                handler: "autumn_{name_snake}_plugin::index".to_owned(),
                source: RouteSource::Plugin("autumn-{name}-plugin".to_owned()),
                middleware: vec![],
                api_version: None,
                status: None,
                sunset_opt_out: None,
            }},
        ];

        let config = ConformanceConfig::new("autumn-{name}-plugin")
            .prefix("/autumn-{name}-plugin")
            .sensitive_route("/autumn-{name}-plugin", "Role: admin required");

        let report = run_conformance(&config, &routes);
        assert!(
            report.passed(),
            "conformance failed:\n{{}}",
            report.to_text_report()
        );
    }}
}}
"#
    );

    plan.create(target_dir.join("Cargo.toml"), cargo_toml_content);
    plan.create(target_dir.join("src/lib.rs"), lib_rs_content);
    plan.create(target_dir.join("README.md"), readme_content);
    plan.create(
        target_dir.join("tests/conformance.rs"),
        conformance_rs_content,
    );

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn plan_creates_plugin_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let target_dir = project_root.join("autumn-foo-plugin");

        let plan = plan_plugin(project_root, "foo", None, Flags::default()).unwrap();

        // Verify Cargo.toml, src/lib.rs, README.md, and tests/conformance.rs are planned for creation
        let paths: std::collections::HashSet<PathBuf> = plan
            .actions
            .iter()
            .map(|a| a.path().to_path_buf())
            .collect();

        assert!(
            paths.contains(&target_dir.join("Cargo.toml")),
            "Should plan Cargo.toml"
        );
        assert!(
            paths.contains(&target_dir.join("src/lib.rs")),
            "Should plan src/lib.rs"
        );
        assert!(
            paths.contains(&target_dir.join("README.md")),
            "Should plan README.md"
        );
        assert!(
            paths.contains(&target_dir.join("tests/conformance.rs")),
            "Should plan tests/conformance.rs"
        );
    }

    #[test]
    fn plan_includes_correct_contents_and_conformance_run() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let target_dir = project_root.join("autumn-foo-plugin");

        let plan = plan_plugin(project_root, "foo", None, Flags::default()).unwrap(); // Check Cargo.toml content
        let cargo_action = plan
            .actions
            .iter()
            .find(|a| a.path() == target_dir.join("Cargo.toml"))
            .unwrap();
        let super::super::emit::Action::Create {
            contents: cargo_content,
            ..
        } = cargo_action
        else {
            panic!("Expected Create action")
        };
        assert!(cargo_content.contains("[package]"));
        assert!(cargo_content.contains("name = \"autumn-foo-plugin\""));
        assert!(cargo_content.contains("edition = \"2024\""));
        assert!(cargo_content.contains("autumn-web = { version = \"0.5\" }"));
        assert!(cargo_content.contains("serde = { version = \"1\", features = [\"derive\"] }"));

        // Check src/lib.rs content
        let lib_action = plan
            .actions
            .iter()
            .find(|a| a.path() == target_dir.join("src/lib.rs"))
            .unwrap();
        let super::super::emit::Action::Create {
            contents: lib_content,
            ..
        } = lib_action
        else {
            panic!("Expected Create action")
        };
        assert!(lib_content.contains("impl Plugin for FooPlugin"));
        assert!(lib_content.contains("fn name(&self) -> Cow<'static, str>"));
        assert!(lib_content.contains("Cow::Borrowed(\"autumn-foo-plugin\")"));
        assert!(lib_content.contains("fn build(self, app: AppBuilder) -> AppBuilder"));
        assert!(lib_content.contains("// app.routes("));
        assert!(lib_content.contains("index"));

        // Check README.md content
        let readme_action = plan
            .actions
            .iter()
            .find(|a| a.path() == target_dir.join("README.md"))
            .unwrap();
        let super::super::emit::Action::Create {
            contents: readme_content,
            ..
        } = readme_action
        else {
            panic!("Expected Create action")
        };
        assert!(readme_content.contains("# autumn-foo-plugin"));
        assert!(readme_content.contains("autumn-foo-plugin = { version = \"0.1.0\" }"));
        assert!(readme_content.contains(".plugin(FooPlugin::new())"));

        // Check tests/conformance.rs content
        let conformance_action = plan
            .actions
            .iter()
            .find(|a| a.path() == target_dir.join("tests/conformance.rs"))
            .unwrap();
        let super::super::emit::Action::Create {
            contents: conformance_content,
            ..
        } = conformance_action
        else {
            panic!("Expected Create action")
        };
        assert!(conformance_content.contains("run_conformance"));
        assert!(conformance_content.contains("ConformanceConfig::new"));

        // Test non-empty directory collision check
        let collision_dir = project_root.join("autumn-bar-plugin");
        fs::create_dir_all(&collision_dir).unwrap();
        fs::write(collision_dir.join("some-file.txt"), "hello").unwrap();

        let err = plan_plugin(project_root, "bar", None, Flags::default()).unwrap_err();
        match err {
            GenerateError::Config(msg) => {
                let expected = format!(
                    "target directory '{}' already exists and is not empty. Use --force to override.",
                    collision_dir.display().to_string().replace('\\', "/")
                );
                assert_eq!(msg, expected);
            }
            _ => panic!("Expected GenerateError::Config, got {err:?}"),
        }

        // If force is true, it should bypass the check and succeed
        let plan_forced = plan_plugin(
            project_root,
            "bar",
            None,
            Flags {
                force: true,
                ..Default::default()
            },
        );
        assert!(plan_forced.is_ok());
    }
}
