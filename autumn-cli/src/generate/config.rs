//! TOML-based generator configuration for `autumn generate scaffold`.
//!
//! Reads `[scaffold.ResourceName]` sections from a project-local TOML file
//! so scaffold metadata can be checked in and reproduced without long CLI
//! invocations.
//!
//! # File format
//!
//! ```toml
//! [scaffold.Bookmark]
//! fields      = ["url:String", "title:String", "tag:String", "alive:bool"]
//! indexes     = ["url", "tag"]
//! validations = ["url=url", "title=length:min=1,max=200"]
//! defaults    = ["alive=true"]
//! queries     = ["find_by_tag:tag", "find_by_alive:alive"]
//! ```
//!
//! # Precedence
//!
//! CLI flags win. If the caller passes any values for a repeated flag (e.g.
//! `--index`), those values completely replace the corresponding TOML list.
//! Fields (positional arguments) follow the same rule: non-empty CLI fields
//! shadow TOML fields.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::GenerateError;
use super::model::ModelOptions;
use super::naming::pascal;
use super::scaffold::ScaffoldOptions;

/// One resource's scaffold metadata from a TOML config file.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScaffoldConfigEntry {
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub indexes: Vec<String>,
    #[serde(default)]
    pub validations: Vec<String>,
    #[serde(default)]
    pub defaults: Vec<String>,
    #[serde(default)]
    pub queries: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct GeneratorConfig {
    #[serde(default)]
    scaffold: HashMap<String, ScaffoldConfigEntry>,
}

/// Read the scaffold config entry for `resource_name` from `config_path`.
///
/// Returns `None` when the file is valid TOML but has no
/// `[scaffold.ResourceName]` section for the requested name.
///
/// # Errors
///
/// - [`GenerateError::Io`] if the file cannot be read.
/// - [`GenerateError::Config`] if the file is not valid TOML.
pub fn read_scaffold_config(
    config_path: &Path,
    resource_name: &str,
) -> Result<Option<ScaffoldConfigEntry>, GenerateError> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", config_path.display())))?;
    let mut config: GeneratorConfig = toml::from_str(&content).map_err(|e| {
        GenerateError::Config(format!("invalid TOML in {}: {e}", config_path.display()))
    })?;
    // Normalize to PascalCase so `bookmark` and `Bookmark` both match
    // `[scaffold.Bookmark]`, consistent with how the generator itself handles
    // resource names passed on the CLI.
    Ok(config.scaffold.remove(pascal(resource_name).as_str()))
}

/// Merge a TOML config entry with CLI-supplied values.
///
/// For each key, if the caller supplied a non-empty CLI slice that value set
/// replaces the TOML list; otherwise the TOML list is kept.
pub fn merge_config_with_cli(
    config: ScaffoldConfigEntry,
    cli_fields: &[String],
    cli_indexes: &[String],
    cli_validations: &[String],
    cli_defaults: &[String],
    cli_queries: &[String],
) -> (Vec<String>, ScaffoldOptions) {
    let pick = |cli: &[String], toml: Vec<String>| -> Vec<String> {
        if cli.is_empty() { toml } else { cli.to_vec() }
    };
    let fields = pick(cli_fields, config.fields);
    let indexes = pick(cli_indexes, config.indexes);
    let validations = pick(cli_validations, config.validations);
    let defaults = pick(cli_defaults, config.defaults);
    let queries = pick(cli_queries, config.queries);
    (
        fields,
        ScaffoldOptions {
            model: ModelOptions {
                indexes,
                validations,
                defaults,
                soft_delete: false,
            },
            queries,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_config(tmp: &TempDir, content: &str) -> std::path::PathBuf {
        let path = tmp.path().join("autumn.generate.toml");
        fs::write(&path, content).unwrap();
        path
    }

    // ── read_scaffold_config ──────────────────────────────────────────────

    #[test]
    fn parse_valid_scaffold_config() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            r#"
[scaffold.Bookmark]
fields      = ["url:String", "title:String", "tag:String", "alive:bool"]
indexes     = ["url", "tag"]
validations = ["url=url", "title=length:min=1,max=200"]
defaults    = ["alive=true"]
queries     = ["find_by_tag:tag", "find_by_alive:alive"]
"#,
        );

        let entry = read_scaffold_config(&path, "Bookmark")
            .unwrap()
            .expect("Bookmark section must be present");

        assert_eq!(
            entry.fields,
            vec!["url:String", "title:String", "tag:String", "alive:bool"]
        );
        assert_eq!(entry.indexes, vec!["url", "tag"]);
        assert_eq!(
            entry.validations,
            vec!["url=url", "title=length:min=1,max=200"]
        );
        assert_eq!(entry.defaults, vec!["alive=true"]);
        assert_eq!(
            entry.queries,
            vec!["find_by_tag:tag", "find_by_alive:alive"]
        );
    }

    #[test]
    fn parse_minimal_config_with_fields_only() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(&tmp, "[scaffold.Post]\nfields = [\"title:String\"]\n");

        let entry = read_scaffold_config(&path, "Post").unwrap().unwrap();
        assert_eq!(entry.fields, vec!["title:String"]);
        assert!(entry.indexes.is_empty());
        assert!(entry.validations.is_empty());
        assert!(entry.defaults.is_empty());
        assert!(entry.queries.is_empty());
    }

    #[test]
    fn snake_case_name_matches_pascal_case_toml_section() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(&tmp, "[scaffold.Bookmark]\nfields = [\"url:String\"]\n");

        // CLI input `bookmark` (snake_case) must find `[scaffold.Bookmark]`.
        let entry = read_scaffold_config(&path, "bookmark")
            .unwrap()
            .expect("snake_case name should resolve to PascalCase section");
        assert_eq!(entry.fields, vec!["url:String"]);
    }

    #[test]
    fn unknown_key_returns_config_error() {
        let tmp = TempDir::new().unwrap();
        // `index` is the wrong key; the correct key is `indexes`.
        let path = write_config(
            &tmp,
            "[scaffold.Post]\nfields = [\"title:String\"]\nindex = [\"title\"]\n",
        );

        let err = read_scaffold_config(&path, "Post").unwrap_err();
        assert!(
            matches!(err, GenerateError::Config(_)),
            "misspelled key should return Config error, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("index"),
            "error should mention the unknown key; got: {msg}"
        );
    }

    #[test]
    fn missing_section_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(&tmp, "[scaffold.Post]\nfields = [\"title:String\"]\n");

        let result = read_scaffold_config(&path, "Bookmark").unwrap();
        assert!(result.is_none(), "unknown resource should return None");
    }

    #[test]
    fn invalid_toml_returns_config_error() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(&tmp, "not valid toml {{{{");

        let err = read_scaffold_config(&path, "Bookmark").unwrap_err();
        assert!(
            matches!(err, GenerateError::Config(_)),
            "expected Config error, got: {err:?}"
        );
    }

    #[test]
    fn missing_file_returns_io_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");

        let err = read_scaffold_config(&path, "Bookmark").unwrap_err();
        assert!(
            matches!(err, GenerateError::Io(_)),
            "expected Io error for missing file, got: {err:?}"
        );
    }

    // ── merge_config_with_cli ─────────────────────────────────────────────

    fn bookmark_entry() -> ScaffoldConfigEntry {
        ScaffoldConfigEntry {
            fields: vec!["url:String".into(), "tag:String".into()],
            indexes: vec!["url".into()],
            validations: vec!["url=url".into()],
            defaults: vec![],
            queries: vec!["find_by_url:url".into()],
        }
    }

    #[test]
    fn merge_uses_toml_when_all_cli_empty() {
        let (fields, opts) = merge_config_with_cli(bookmark_entry(), &[], &[], &[], &[], &[]);
        assert_eq!(fields, vec!["url:String", "tag:String"]);
        assert_eq!(opts.model.indexes, vec!["url"]);
        assert_eq!(opts.model.validations, vec!["url=url"]);
        assert!(opts.model.defaults.is_empty());
        assert_eq!(opts.queries, vec!["find_by_url:url"]);
    }

    #[test]
    fn merge_cli_fields_override_toml_fields() {
        let (fields, _) = merge_config_with_cli(
            bookmark_entry(),
            &["title:String".into(), "body:Text".into()],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(fields, vec!["title:String", "body:Text"]);
    }

    #[test]
    fn merge_cli_indexes_override_toml_indexes() {
        let (_, opts) =
            merge_config_with_cli(bookmark_entry(), &[], &["tag".into()], &[], &[], &[]);
        assert_eq!(opts.model.indexes, vec!["tag"]);
    }

    #[test]
    fn merge_cli_validations_override_toml_validations() {
        let (_, opts) =
            merge_config_with_cli(bookmark_entry(), &[], &[], &["url=email".into()], &[], &[]);
        assert_eq!(opts.model.validations, vec!["url=email"]);
    }

    #[test]
    fn merge_cli_defaults_override_toml_defaults() {
        let mut entry = bookmark_entry();
        entry.defaults = vec!["url=example.com".into()];
        let (_, opts) = merge_config_with_cli(entry, &[], &[], &[], &["tag=general".into()], &[]);
        assert_eq!(opts.model.defaults, vec!["tag=general"]);
    }

    #[test]
    fn merge_cli_queries_override_toml_queries() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &[],
            &[],
            &[],
            &["find_by_tag:tag".into()],
        );
        assert_eq!(opts.queries, vec!["find_by_tag:tag"]);
    }

    #[test]
    fn merge_empty_cli_keeps_empty_toml() {
        let entry = ScaffoldConfigEntry::default();
        let (fields, opts) = merge_config_with_cli(entry, &[], &[], &[], &[], &[]);
        assert!(fields.is_empty());
        assert!(opts.model.indexes.is_empty());
        assert!(opts.model.validations.is_empty());
        assert!(opts.model.defaults.is_empty());
        assert!(opts.queries.is_empty());
    }
}
