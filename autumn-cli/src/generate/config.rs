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

/// The conventional config file name that `autumn generate` auto-discovers in
/// the project root. Also used as the default path for `--config`.
pub const GENERATE_CONFIG_FILENAME: &str = "autumn.generate.toml";

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::GenerateError;
use super::dsl::IdType;
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
    #[serde(default)]
    pub soft_delete: bool,
    #[serde(default)]
    pub api: bool,
    #[serde(default)]
    pub sharded: bool,
    #[serde(default)]
    pub shard_key: Option<String>,
    /// Primary-key type for this resource (`"uuid"` or `"bigint"`).
    /// Inherits from `[generate] id` when absent.
    #[serde(default)]
    pub id: Option<String>,
}

/// Project-level generator defaults, read from `[generate]` in the config file.
#[derive(Debug, Clone, Default, Deserialize)]
struct GenerateDefaults {
    /// Default primary-key type (`"uuid"` or `"bigint"`).
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct GeneratorConfig {
    #[serde(default)]
    scaffold: HashMap<String, ScaffoldConfigEntry>,
    #[serde(default)]
    generate: GenerateDefaults,
}

/// Read the scaffold config entry for `resource_name` from `config_path`.
///
/// Returns `None` when the file is valid TOML but has no
/// `[scaffold.ResourceName]` section for the requested name.
///
/// The `[generate] id` project default is propagated into the returned entry's
/// `id` field so that callers do not need to handle the two-level default
/// separately.
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
    let mut entry = config.scaffold.remove(pascal(resource_name).as_str());
    // Propagate [generate] id default when the per-resource section doesn't override it.
    if let Some(ref mut e) = entry {
        if e.id.is_none() {
            e.id = config.generate.id.clone();
        }
    }
    Ok(entry)
}

/// Like [`read_scaffold_config`] but always returns a [`ScaffoldConfigEntry`]
/// rather than `Option`.
///
/// When no `[scaffold.ResourceName]` section exists the function returns a
/// default entry, applying any `[generate] id` project default so that a
/// top-level `[generate]` section is honoured even without a per-resource
/// section.
///
/// Use this instead of [`read_scaffold_config`] when a missing section should
/// silently fall back to project defaults rather than being treated as an error
/// (e.g. auto-discovered project config and `--config` with only `[generate]`).
///
/// # Errors
///
/// - [`GenerateError::Io`] if the file cannot be read.
/// - [`GenerateError::Config`] if the file is not valid TOML.
pub fn read_scaffold_config_or_defaults(
    config_path: &Path,
    resource_name: &str,
) -> Result<ScaffoldConfigEntry, GenerateError> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", config_path.display())))?;
    let mut config: GeneratorConfig = toml::from_str(&content).map_err(|e| {
        GenerateError::Config(format!("invalid TOML in {}: {e}", config_path.display()))
    })?;
    let mut entry = config
        .scaffold
        .remove(pascal(resource_name).as_str())
        .unwrap_or_default();
    if entry.id.is_none() {
        entry.id = config.generate.id;
    }
    Ok(entry)
}

/// Read the project-level `[generate]` defaults from `config_path`, returning
/// the `IdType` default if one is set.
///
/// # Errors
///
/// - [`GenerateError::Io`] if the file cannot be read.
/// - [`GenerateError::Config`] if the file is not valid TOML or `id` is invalid.
pub fn read_generate_defaults(config_path: &Path) -> Result<IdType, GenerateError> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", config_path.display())))?;
    let config: GeneratorConfig = toml::from_str(&content).map_err(|e| {
        GenerateError::Config(format!("invalid TOML in {}: {e}", config_path.display()))
    })?;
    if let Some(id_str) = config.generate.id {
        IdType::parse(&id_str)
    } else {
        Ok(IdType::default())
    }
}

/// Merge a TOML config entry with CLI-supplied values.
///
/// For each key, if the caller supplied a non-empty CLI slice that value set
/// replaces the TOML list; otherwise the TOML list is kept.
///
/// The `cli_id` parameter follows the precedence rule:
///   `--id` CLI > `[scaffold.X] id` TOML > `[generate] id` TOML > BigSerial.
///
/// # Errors
/// Returns [`GenerateError::Config`] if any `--id` value is unrecognised.
#[allow(clippy::too_many_arguments)]
pub fn merge_config_with_cli(
    config: ScaffoldConfigEntry,
    cli_fields: &[String],
    cli_indexes: &[String],
    cli_validations: &[String],
    cli_defaults: &[String],
    cli_queries: &[String],
    cli_soft_delete: bool,
    cli_api: bool,
    cli_sharded: bool,
    cli_shard_key: Option<&str>,
    cli_id: Option<&str>,
) -> Result<(Vec<String>, ScaffoldOptions), GenerateError> {
    let pick = |cli: &[String], toml: Vec<String>| -> Vec<String> {
        if cli.is_empty() { toml } else { cli.to_vec() }
    };
    let fields = pick(cli_fields, config.fields);
    let indexes = pick(cli_indexes, config.indexes);
    let validations = pick(cli_validations, config.validations);
    let defaults = pick(cli_defaults, config.defaults);
    let queries = pick(cli_queries, config.queries);
    // CLI flag wins; TOML config enables it when present.
    let soft_delete = cli_soft_delete || config.soft_delete;
    let api = cli_api || config.api;
    let sharded = cli_sharded || config.sharded;
    let shard_key = cli_shard_key.map(str::to_owned).or(config.shard_key);
    // Precedence: CLI > per-resource TOML > project-default TOML > BigSerial.
    let id_type = if let Some(s) = cli_id {
        IdType::parse(s)?
    } else if let Some(ref s) = config.id {
        IdType::parse(s)?
    } else {
        IdType::default()
    };
    Ok((
        fields,
        ScaffoldOptions {
            model: ModelOptions {
                indexes,
                validations,
                defaults,
                soft_delete,
                sharded,
                shard_key,
                id_type,
            },
            queries,
            api,
        },
    ))
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
            soft_delete: false,
            api: false,
            sharded: false,
            shard_key: None,
            id: None,
        }
    }

    fn merge(entry: ScaffoldConfigEntry) -> (Vec<String>, ScaffoldOptions) {
        merge_config_with_cli(entry, &[], &[], &[], &[], &[], false, false, false, None, None).unwrap()
    }

    #[test]
    fn merge_uses_toml_when_all_cli_empty() {
        let (fields, opts) = merge(bookmark_entry());
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
            false,
            false,
            false,
            None,
            None,
        ).unwrap();
        assert_eq!(fields, vec!["title:String", "body:Text"]);
    }

    #[test]
    fn merge_cli_indexes_override_toml_indexes() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &["tag".into()],
            &[],
            &[],
            &[],
            false,
            false,
            false,
            None,
            None,
        ).unwrap();
        assert_eq!(opts.model.indexes, vec!["tag"]);
    }

    #[test]
    fn merge_cli_validations_override_toml_validations() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &[],
            &["url=email".into()],
            &[],
            &[],
            false,
            false,
            false,
            None,
            None,
        ).unwrap();
        assert_eq!(opts.model.validations, vec!["url=email"]);
    }

    #[test]
    fn merge_cli_defaults_override_toml_defaults() {
        let mut entry = bookmark_entry();
        entry.defaults = vec!["url=example.com".into()];
        let (_, opts) = merge_config_with_cli(
            entry,
            &[],
            &[],
            &[],
            &["tag=general".into()],
            &[],
            false,
            false,
            false,
            None,
            None,
        ).unwrap();
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
            false,
            false,
            false,
            None,
            None,
        ).unwrap();
        assert_eq!(opts.queries, vec!["find_by_tag:tag"]);
    }

    #[test]
    fn merge_empty_cli_keeps_empty_toml() {
        let entry = ScaffoldConfigEntry::default();
        let (fields, opts) =
            merge_config_with_cli(entry, &[], &[], &[], &[], &[], false, false, false, None, None).unwrap();
        assert!(fields.is_empty());
        assert!(opts.model.indexes.is_empty());
        assert!(opts.model.validations.is_empty());
        assert!(opts.model.defaults.is_empty());
        assert!(opts.queries.is_empty());
    }

    #[test]
    fn merge_cli_soft_delete_flag_wins() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &[],
            &[],
            &[],
            &[],
            true,
            false,
            false,
            None,
            None,
        ).unwrap();
        assert!(
            opts.model.soft_delete,
            "cli_soft_delete=true must set soft_delete on the merged options"
        );
    }

    #[test]
    fn merge_toml_soft_delete_propagates() {
        let mut entry = bookmark_entry();
        entry.soft_delete = true;
        let (_, opts) = merge(entry);
        assert!(
            opts.model.soft_delete,
            "soft_delete=true in TOML config must propagate when CLI flag is false"
        );
    }

    #[test]
    fn merge_soft_delete_false_when_both_unset() {
        let (_, opts) = merge(bookmark_entry());
        assert!(
            !opts.model.soft_delete,
            "soft_delete must be false when neither CLI nor TOML sets it"
        );
    }

    #[test]
    fn parse_scaffold_config_with_soft_delete() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[scaffold.Article]\nfields = [\"title:String\"]\nsoft_delete = true\n",
        );
        let entry = read_scaffold_config(&path, "Article")
            .unwrap()
            .expect("Article section must be present");
        assert!(
            entry.soft_delete,
            "soft_delete = true in TOML must be parsed into ScaffoldConfigEntry"
        );
    }

    #[test]
    fn merge_cli_api_flag_wins() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &[],
            &[],
            &[],
            &[],
            false,
            true,
            false,
            None,
            None,
        ).unwrap();
        assert!(opts.api);
    }

    #[test]
    fn merge_toml_api_propagates() {
        let mut entry = bookmark_entry();
        entry.api = true;
        let (_, opts) = merge(entry);
        assert!(opts.api);
    }

    #[test]
    fn parse_scaffold_config_with_api() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[scaffold.Article]\nfields = [\"title:String\"]\napi = true\n",
        );
        let entry = read_scaffold_config(&path, "Article")
            .unwrap()
            .expect("Article section must be present");
        assert!(
            entry.api,
            "api = true in TOML must be parsed into ScaffoldConfigEntry"
        );
    }

    #[test]
    fn merge_cli_sharded_flag_wins() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(),
            &[],
            &[],
            &[],
            &[],
            &[],
            false,
            false,
            true,
            None,
            None,
        ).unwrap();
        assert!(
            opts.model.sharded,
            "cli_sharded=true must set sharded on the merged options"
        );
    }

    #[test]
    fn merge_toml_sharded_propagates() {
        let mut entry = bookmark_entry();
        entry.sharded = true;
        let (_, opts) = merge(entry);
        assert!(
            opts.model.sharded,
            "sharded=true in TOML config must propagate when CLI flag is false"
        );
    }

    #[test]
    fn merge_shard_key_cli_overrides_toml() {
        let mut entry = bookmark_entry();
        entry.shard_key = Some("tenant_id".into());
        let (_, opts) = merge_config_with_cli(
            entry,
            &[],
            &[],
            &[],
            &[],
            &[],
            false,
            false,
            false,
            Some("user_id"),
            None,
        ).unwrap();
        assert_eq!(
            opts.model.shard_key.as_deref(),
            Some("user_id"),
            "CLI shard_key must override TOML shard_key"
        );
    }

    #[test]
    fn merge_shard_key_toml_used_when_no_cli() {
        let mut entry = bookmark_entry();
        entry.shard_key = Some("org_id".into());
        let (_, opts) = merge(entry);
        assert_eq!(
            opts.model.shard_key.as_deref(),
            Some("org_id"),
            "TOML shard_key must propagate when no CLI shard_key is given"
        );
    }

    #[test]
    fn parse_scaffold_config_with_sharded() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[scaffold.Account]\nfields = [\"tenant_id:i64\"]\nsharded = true\nshard_key = \"tenant_id\"\n",
        );
        let entry = read_scaffold_config(&path, "Account")
            .unwrap()
            .expect("Account section must be present");
        assert!(
            entry.sharded,
            "sharded = true in TOML must be parsed into ScaffoldConfigEntry"
        );
        assert_eq!(
            entry.shard_key.as_deref(),
            Some("tenant_id"),
            "shard_key in TOML must be parsed into ScaffoldConfigEntry"
        );
    }

    // ── id_type (issue #1400) ──────────────────────────────────────────────

    #[test]
    fn merge_cli_id_uuid_sets_uuid_id_type() {
        let (_, opts) = merge_config_with_cli(
            bookmark_entry(), &[], &[], &[], &[], &[], false, false, false, None, Some("uuid"),
        ).unwrap();
        assert_eq!(opts.model.id_type, IdType::Uuid, "cli --id uuid must set Uuid id_type");
    }

    #[test]
    fn merge_toml_id_uuid_propagates() {
        let mut entry = bookmark_entry();
        entry.id = Some("uuid".into());
        let (_, opts) = merge(entry);
        assert_eq!(opts.model.id_type, IdType::Uuid, "[scaffold.X] id = 'uuid' must propagate");
    }

    #[test]
    fn merge_cli_id_overrides_toml_id() {
        let mut entry = bookmark_entry();
        entry.id = Some("uuid".into());
        let (_, opts) = merge_config_with_cli(
            entry, &[], &[], &[], &[], &[], false, false, false, None, Some("bigint"),
        ).unwrap();
        assert_eq!(opts.model.id_type, IdType::BigSerial, "CLI --id bigint must override TOML id = 'uuid'");
    }

    #[test]
    fn merge_default_id_type_is_bigserial() {
        let (_, opts) = merge(bookmark_entry());
        assert_eq!(opts.model.id_type, IdType::BigSerial, "default id_type must be BigSerial (AC4)");
    }

    #[test]
    fn merge_cli_bad_id_returns_error() {
        let err = merge_config_with_cli(
            bookmark_entry(), &[], &[], &[], &[], &[], false, false, false, None, Some("guid"),
        ).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("guid"), "error must mention the bad value: {msg}");
        assert!(msg.contains("uuid"), "error must list accepted values: {msg}");
    }

    #[test]
    fn project_default_id_propagates_via_generate_section() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[generate]\nid = \"uuid\"\n\n[scaffold.Post]\nfields = [\"title:String\"]\n",
        );
        let defaults_id = read_generate_defaults(&path).unwrap();
        assert_eq!(defaults_id, IdType::Uuid, "[generate] id = 'uuid' must be read as Uuid");
    }

    #[test]
    fn project_default_propagates_into_scaffold_entry() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[generate]\nid = \"uuid\"\n\n[scaffold.Post]\nfields = [\"title:String\"]\n",
        );
        let entry = read_scaffold_config(&path, "Post").unwrap().unwrap();
        assert_eq!(
            entry.id.as_deref(),
            Some("uuid"),
            "[generate] id must propagate into scaffold entry id when unset"
        );
    }

    #[test]
    fn per_resource_id_overrides_project_default() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            &tmp,
            "[generate]\nid = \"uuid\"\n\n[scaffold.Post]\nfields = [\"title:String\"]\nid = \"bigint\"\n",
        );
        let entry = read_scaffold_config(&path, "Post").unwrap().unwrap();
        assert_eq!(
            entry.id.as_deref(),
            Some("bigint"),
            "per-resource id must not be overridden by [generate] default"
        );
    }
}
