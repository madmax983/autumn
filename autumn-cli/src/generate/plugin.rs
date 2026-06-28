use super::GenerateError;
use super::emit::Plan;
use std::path::Path;

/// Plans the generation of a conformant plugin crate.
#[allow(clippy::unnecessary_wraps)]
pub fn plan_plugin(target_dir: &Path, _plugin_name: &str) -> Result<Plan, GenerateError> {
    let mut plan = Plan::new(target_dir);
    plan.create(target_dir.join("Cargo.toml"), "");
    plan.create(target_dir.join("src/lib.rs"), "");
    plan.create(target_dir.join("README.md"), "");
    plan.create(target_dir.join("tests/conformance.rs"), "");
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn plan_creates_plugin_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let target_dir = temp_dir.path().join("autumn-foo-plugin");

        let plan = plan_plugin(&target_dir, "foo").unwrap();

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
}
