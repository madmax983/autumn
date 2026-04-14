//! Live-feed bus configuration for embedded and distributed reddit-clone runs.
//!
//! The durable `live_feed_events` table remains the replay source of truth.
//! This config only selects how processes nudge each other to replay new rows.

use std::path::{Path, PathBuf};

use autumn_web::config::{Env, OsEnv};
use serde::Deserialize;
use thiserror::Error;

fn default_channel() -> String {
    "reddit_live_feed".to_owned()
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveFeedBusKind {
    #[default]
    PostgresNotify,
    #[serde(rename = "redis_pubsub")]
    RedisPubSub,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LiveFeedBusConfig {
    #[serde(default)]
    pub kind: LiveFeedBusKind,
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default = "default_channel")]
    pub channel: String,
}

impl Default for LiveFeedBusConfig {
    fn default() -> Self {
        Self {
            kind: LiveFeedBusKind::PostgresNotify,
            redis_url: None,
            channel: default_channel(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct DistributedConfig {
    #[serde(default)]
    live_feed_bus: LiveFeedBusConfig,
}

impl LiveFeedBusConfig {
    pub fn load() -> Result<Self, LiveFeedBusConfigLoadError> {
        Self::load_with_env(&OsEnv)
    }

    pub fn load_with_env(env: &dyn Env) -> Result<Self, LiveFeedBusConfigLoadError> {
        let manifest_dir = resolve_manifest_dir(env);
        let profile = resolve_profile(env);

        Self::load_from_dir(manifest_dir, profile.as_deref())
    }

    pub fn load_from_dir(
        manifest_dir: impl AsRef<Path>,
        profile: Option<&str>,
    ) -> Result<Self, LiveFeedBusConfigLoadError> {
        let manifest_dir = manifest_dir.as_ref();
        let base_path = manifest_dir.join("autumn.toml");
        let mut distributed =
            load_distributed_section(&base_path)?.unwrap_or_else(empty_distributed_section);

        if let Some(profile) = profile {
            let overlay_path = manifest_dir.join(format!("autumn-{profile}.toml"));
            if let Some(overlay) = load_distributed_section(&overlay_path)? {
                deep_merge(&mut distributed, overlay);
            }
        }

        let distributed: DistributedConfig = toml::from_str(
            &toml::to_string(&distributed)
                .expect("distributed live-feed bus config should serialize"),
        )
        .map_err(|source| LiveFeedBusConfigLoadError::Parse {
            path: base_path,
            source: Box::new(source),
        })?;

        Ok(distributed.live_feed_bus)
    }
}

#[derive(Debug, Error)]
pub enum LiveFeedBusConfigLoadError {
    #[error("failed to read live-feed bus config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse live-feed bus config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
}

fn resolve_manifest_dir(env: &dyn Env) -> PathBuf {
    env.var("AUTUMN_MANIFEST_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn resolve_profile(env: &dyn Env) -> Option<String> {
    if let Ok(profile) = env.var("AUTUMN_PROFILE") {
        if !profile.is_empty() {
            return Some(profile);
        }
    }

    for (index, arg) in std::env::args().enumerate() {
        if arg == "--profile" {
            if let Some(profile) = std::env::args().nth(index + 1) {
                if !profile.is_empty() {
                    return Some(profile);
                }
            }
        }
        if let Some(profile) = arg.strip_prefix("--profile=") {
            if !profile.is_empty() {
                return Some(profile.to_owned());
            }
        }
    }

    match env.var("AUTUMN_IS_DEBUG").ok().as_deref() {
        Some("1") => Some("dev".to_owned()),
        Some("0") => Some("prod".to_owned()),
        _ => None,
    }
}

fn load_distributed_section(
    path: &Path,
) -> Result<Option<toml::Value>, LiveFeedBusConfigLoadError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let table: toml::Table =
                toml::from_str(&contents).map_err(|source| LiveFeedBusConfigLoadError::Parse {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })?;
            Ok(toml::Value::Table(table).get("distributed").cloned())
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LiveFeedBusConfigLoadError::Io {
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

fn empty_distributed_section() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}
