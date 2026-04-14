use serde::Deserialize;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

fn default_pool_size() -> usize {
    10
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DistributedConfig {
    #[serde(default)]
    pub database: DistributedDatabaseConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DistributedDatabaseConfig {
    #[serde(default)]
    pub primary_url: Option<String>,
    #[serde(default)]
    pub replica_url: Option<String>,
    #[serde(default = "default_pool_size")]
    pub primary_pool_size: usize,
    #[serde(default = "default_pool_size")]
    pub replica_pool_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingDistributedDatabaseUrls {
    missing_primary: bool,
    missing_replica: bool,
}

impl MissingDistributedDatabaseUrls {
    #[must_use]
    pub fn primary_and_replica() -> Self {
        Self {
            missing_primary: true,
            missing_replica: true,
        }
    }

    #[must_use]
    pub fn primary() -> Self {
        Self {
            missing_primary: true,
            missing_replica: false,
        }
    }

    #[must_use]
    pub fn replica() -> Self {
        Self {
            missing_primary: false,
            missing_replica: true,
        }
    }
}

impl fmt::Display for MissingDistributedDatabaseUrls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.missing_primary, self.missing_replica) {
            (true, true) => f.write_str("primary and replica database URLs are required"),
            (true, false) => f.write_str("primary database URL is required"),
            (false, true) => f.write_str("replica database URL is required"),
            (false, false) => f.write_str("database URLs are required"),
        }
    }
}

impl Error for MissingDistributedDatabaseUrls {}

impl Default for DistributedDatabaseConfig {
    fn default() -> Self {
        Self {
            primary_url: None,
            replica_url: None,
            primary_pool_size: default_pool_size(),
            replica_pool_size: default_pool_size(),
        }
    }
}

impl DistributedConfig {
    pub fn load() -> Result<Self, DistributedConfigLoadError> {
        use autumn_web::config::{Env as _, OsEnv};
        let env = OsEnv;
        let manifest_dir =
            resolve_manifest_dir(env.var("AUTUMN_MANIFEST_DIR").ok().as_deref());
        let profile = resolve_runtime_profile(
            env.var("AUTUMN_PROFILE").ok().as_deref(),
            &std::env::args().collect::<Vec<_>>(),
            env.var("AUTUMN_IS_DEBUG").ok().as_deref(),
        );

        Self::load_from_dir(manifest_dir, profile.as_deref())
    }

    pub fn load_from_dir(
        manifest_dir: impl AsRef<Path>,
        profile: Option<&str>,
    ) -> Result<Self, DistributedConfigLoadError> {
        let manifest_dir = manifest_dir.as_ref();
        let base_path = manifest_dir.join("autumn.toml");
        let mut merged = load_distributed_section(&base_path)?.ok_or(
            DistributedConfigLoadError::MissingSection {
                path: base_path.clone(),
            },
        )?;

        if let Some(profile) = profile {
            let overlay_path = manifest_dir.join(format!("autumn-{profile}.toml"));
            if let Some(overlay) = load_distributed_section(&overlay_path)? {
                deep_merge(&mut merged, overlay);
            }
        }

        let toml_str = toml::to_string(&merged).expect("distributed config merge should serialize");
        toml::from_str(&toml_str).map_err(|source| DistributedConfigLoadError::Parse {
            path: base_path,
            source: Box::new(source),
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn from_urls(primary_url: &str, replica_url: &str) -> Self {
        Self {
            database: DistributedDatabaseConfig {
                primary_url: Some(primary_url.to_owned()),
                replica_url: Some(replica_url.to_owned()),
                ..DistributedDatabaseConfig::default()
            },
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_pool_sizes(mut self, primary_pool_size: usize, replica_pool_size: usize) -> Self {
        self.database.primary_pool_size = primary_pool_size;
        self.database.replica_pool_size = replica_pool_size;
        self
    }
}

impl DistributedDatabaseConfig {
    pub fn urls(&self) -> Result<(&str, &str), MissingDistributedDatabaseUrls> {
        match (self.primary_url.as_deref(), self.replica_url.as_deref()) {
            (Some(primary_url), Some(replica_url)) => Ok((primary_url, replica_url)),
            (None, None) => Err(MissingDistributedDatabaseUrls::primary_and_replica()),
            (None, Some(_)) => Err(MissingDistributedDatabaseUrls::primary()),
            (Some(_), None) => Err(MissingDistributedDatabaseUrls::replica()),
        }
    }
}

#[must_use]
pub fn resolve_manifest_dir(manifest_dir_env: Option<&str>) -> PathBuf {
    manifest_dir_env
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

#[must_use]
pub fn resolve_runtime_profile(
    profile_env: Option<&str>,
    args: &[String],
    is_debug_env: Option<&str>,
) -> Option<String> {
    if let Some(profile) = profile_env.filter(|value| !value.is_empty()) {
        return Some(profile.to_owned());
    }

    for (index, arg) in args.iter().enumerate() {
        if arg == "--profile" {
            if let Some(profile) = args.get(index + 1).filter(|value| !value.is_empty()) {
                return Some(profile.clone());
            }
        }
        if let Some(profile) = arg.strip_prefix("--profile=") {
            if !profile.is_empty() {
                return Some(profile.to_owned());
            }
        }
    }

    match is_debug_env {
        Some("1") => Some("dev".to_owned()),
        Some("0") => Some("prod".to_owned()),
        _ => None,
    }
}

#[derive(Debug)]
pub enum DistributedConfigLoadError {
    MissingSection {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
}

impl fmt::Display for DistributedConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSection { path } => {
                write!(f, "missing [distributed] section in {}", path.display())
            }
            Self::Io { path, source } => {
                write!(
                    f,
                    "failed to read distributed config {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(
                    f,
                    "failed to parse distributed config {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for DistributedConfigLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MissingSection { .. } => None,
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source.as_ref()),
        }
    }
}

fn load_distributed_section(
    path: &Path,
) -> Result<Option<toml::Value>, DistributedConfigLoadError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let file_config: toml::Value =
                contents
                    .parse()
                    .map_err(|source| DistributedConfigLoadError::Parse {
                        path: path.to_path_buf(),
                        source: Box::new(source),
                    })?;
            Ok(file_config.get("distributed").cloned())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DistributedConfigLoadError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::{DistributedConfig, resolve_manifest_dir, resolve_runtime_profile};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for a test dir name")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn parses_primary_and_replica_urls_from_toml() {
        let temp_dir = unique_temp_dir("autumn-bookmarks-distributed-parse");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");

        fs::write(
            temp_dir.join("autumn.toml"),
            r#"
                [distributed.database]
                primary_url = "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary"
                replica_url = "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica"
                primary_pool_size = 7
                replica_pool_size = 3
            "#,
        )
        .expect("base config should be written");

        let config = DistributedConfig::load_from_dir(Path::new(&temp_dir), None)
            .expect("distributed config should parse");

        assert_eq!(
            config.database.primary_url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary")
        );
        assert_eq!(
            config.database.replica_url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica")
        );
        assert_eq!(config.database.primary_pool_size, 7);
        assert_eq!(config.database.replica_pool_size, 3);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resolves_runtime_profile_with_autumn_precedence() {
        let args = vec![
            "bookmarks-distributed".to_owned(),
            "--profile=prod".to_owned(),
        ];

        assert_eq!(
            resolve_runtime_profile(Some("staging"), &args, Some("1")),
            Some("staging".to_owned())
        );
        assert_eq!(
            resolve_runtime_profile(None, &args, Some("1")),
            Some("prod".to_owned())
        );
        assert_eq!(
            resolve_runtime_profile(None, &["bookmarks-distributed".to_owned()], Some("1")),
            Some("dev".to_owned())
        );
        assert_eq!(
            resolve_runtime_profile(None, &["bookmarks-distributed".to_owned()], Some("0")),
            Some("prod".to_owned())
        );
    }

    #[test]
    fn resolves_manifest_dir_override_when_present() {
        let temp_dir = unique_temp_dir("autumn-bookmarks-distributed-manifest");
        assert_eq!(
            resolve_manifest_dir(Some(temp_dir.to_str().expect("temp dir should be utf-8"))),
            temp_dir
        );
    }

    #[test]
    fn loads_layered_distributed_section_from_example_files() {
        let temp_dir = unique_temp_dir("autumn-bookmarks-distributed-config");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");

        fs::write(
            temp_dir.join("autumn.toml"),
            r#"
                [distributed.database]
                primary_url = "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary"
                replica_url = "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica"
                primary_pool_size = 4
                replica_pool_size = 2
            "#,
        )
        .expect("base config should be written");
        fs::write(
            temp_dir.join("autumn-staging.toml"),
            r#"
                [distributed.database]
                primary_pool_size = 7
                replica_pool_size = 3
            "#,
        )
        .expect("profile config should be written");

        let config = DistributedConfig::load_from_dir(Path::new(&temp_dir), Some("staging"))
            .expect("distributed config should load from layered files");

        assert_eq!(
            config.database.primary_url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary")
        );
        assert_eq!(
            config.database.replica_url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica")
        );
        assert_eq!(config.database.primary_pool_size, 7);
        assert_eq!(config.database.replica_pool_size, 3);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn missing_distributed_section_is_an_error() {
        let temp_dir = unique_temp_dir("autumn-bookmarks-distributed-config-missing");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");

        fs::write(temp_dir.join("autumn.toml"), "[server]\nport = 3000\n")
            .expect("base config should be written");

        let error = DistributedConfig::load_from_dir(Path::new(&temp_dir), None)
            .expect_err("missing distributed section should fail");

        assert!(error.to_string().contains("distributed"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
