//! Configuration for the [`storage`](super) module.
//!
//! Exposes the `[storage]` TOML section plus environment variable
//! overrides under `AUTUMN_STORAGE__*`. Profile-aware defaults mirror
//! the [`session`](crate::session) module: `dev` gets a working
//! `Local` backend out of the box, `prod` fails fast on `local` unless
//! the operator explicitly opts in via `allow_local_in_production`.

use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Top-level `[storage]` configuration section.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Selected backend.
    #[serde(default)]
    pub backend: StorageBackend,

    /// Stable identifier for the configured provider, written into every
    /// [`Blob`](super::Blob) so applications can spot mismatches when
    /// the framework's storage backend changes.
    #[serde(default = "default_provider")]
    pub default_provider: String,

    /// Allow `backend = "local"` in production profiles instead of
    /// failing fast at startup. Required when running a single-replica
    /// deployment where local disk is acceptable.
    #[serde(default)]
    pub allow_local_in_production: bool,

    /// Configuration for the [`Local`](super::LocalBlobStore) backend.
    #[serde(default)]
    pub local: StorageLocalConfig,

    /// Configuration for the S3 backend.
    #[serde(default)]
    pub s3: StorageS3Config,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::default(),
            default_provider: default_provider(),
            allow_local_in_production: false,
            local: StorageLocalConfig::default(),
            s3: StorageS3Config::default(),
        }
    }
}

/// Selectable storage backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum StorageBackend {
    /// No blob store mounted; [`AppState::extension::<BlobStoreState>()`](crate::AppState::extension)
    /// returns `None`.
    #[default]
    Disabled,
    /// Local disk backend. Suitable for single-replica deployments and
    /// the default for `dev`.
    Local,
    /// S3-compatible backend. Requires the `storage-s3` cargo feature.
    S3,
}

impl StorageBackend {
    /// Parse a backend name from an environment variable value.
    #[must_use]
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "disabled" | "off" | "none" => Some(Self::Disabled),
            "local" => Some(Self::Local),
            "s3" => Some(Self::S3),
            _ => None,
        }
    }
}

/// Configuration for the local-disk backend.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageLocalConfig {
    /// Root directory on disk where blobs are stored. Created on
    /// startup if it does not exist.
    #[serde(default = "default_local_root")]
    pub root: PathBuf,

    /// HTTP path the framework mounts to serve signed blob URLs.
    /// Default `/_blobs`.
    #[serde(default = "default_local_mount_path")]
    pub mount_path: String,

    /// Default URL expiry (seconds) used by
    /// [`BlobStore::presigned_url`](super::BlobStore::presigned_url) when
    /// the caller does not supply one. Falls back to 15 minutes.
    #[serde(default = "default_local_url_expiry_secs")]
    pub default_url_expiry_secs: u64,

    /// HMAC signing key. Falls back to the environment variable
    /// `AUTUMN_STORAGE__LOCAL__SIGNING_KEY`. If neither is set, a random
    /// process-local key is generated at startup — fine for a
    /// single-replica `dev` setup, **never** OK in `prod`.
    #[serde(default)]
    pub signing_key: Option<String>,
}

impl Default for StorageLocalConfig {
    fn default() -> Self {
        Self {
            root: default_local_root(),
            mount_path: default_local_mount_path(),
            default_url_expiry_secs: default_local_url_expiry_secs(),
            signing_key: None,
        }
    }
}

/// Configuration for the S3-compatible backend.
///
/// All fields are optional in TOML so callers can populate sensitive
/// values via environment variable indirection (`*_env`). Concrete
/// validation lives in [`StorageConfig::backend_plan`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StorageS3Config {
    /// Target bucket.
    #[serde(default)]
    pub bucket: Option<String>,

    /// AWS region or region-shaped string accepted by your S3-compatible
    /// provider (R2 uses `auto`).
    #[serde(default)]
    pub region: Option<String>,

    /// Custom endpoint URL. Required for non-AWS providers (R2,
    /// `MinIO`, `DigitalOcean` Spaces, Wasabi). Leave unset for AWS.
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Public base URL used as the prefix for
    /// [`presigned_url`](super::BlobStore::presigned_url). Defaults to
    /// the configured `endpoint` when set, otherwise the SDK default.
    #[serde(default)]
    pub public_base_url: Option<String>,

    /// Environment variable to read the access-key from.
    #[serde(default)]
    pub access_key_id_env: Option<String>,

    /// Environment variable to read the secret access key from.
    #[serde(default)]
    pub secret_access_key_env: Option<String>,

    /// Path-style addressing toggle (R2 / `MinIO` need this `true`).
    #[serde(default)]
    pub force_path_style: bool,

    /// Default URL expiry (seconds). Falls back to 15 minutes.
    #[serde(default = "default_s3_url_expiry_secs")]
    pub default_url_expiry_secs: u64,
}

fn default_provider() -> String {
    "default".to_owned()
}

fn default_local_root() -> PathBuf {
    PathBuf::from("target/blobs")
}

fn default_local_mount_path() -> String {
    "/_blobs".to_owned()
}

const fn default_local_url_expiry_secs() -> u64 {
    15 * 60
}

const fn default_s3_url_expiry_secs() -> u64 {
    15 * 60
}

/// Resolved plan describing how the framework should initialize the
/// configured storage backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageBackendPlan {
    /// No backend installed.
    Disabled,
    /// Provision a local-disk backend with the given options.
    Local {
        /// Stable provider id recorded on every [`Blob`](super::Blob).
        provider_id: String,
        /// Root directory to write blobs under.
        root: PathBuf,
        /// HTTP path to mount the serving route at.
        mount_path: String,
        /// Default URL expiry (seconds) when callers do not supply one.
        default_url_expiry_secs: u64,
        /// Whether to log a warning because local storage is being used
        /// in production without explicit acknowledgement.
        warn_in_production: bool,
    },
    /// Provision an S3-compatible backend.
    S3 {
        /// Stable provider id.
        provider_id: String,
        /// Bucket name.
        bucket: String,
        /// Region.
        region: String,
        /// Custom endpoint, when set.
        endpoint: Option<String>,
        /// Public base URL for presigned URLs, when set.
        public_base_url: Option<String>,
        /// Path-style addressing.
        force_path_style: bool,
        /// Default URL expiry (seconds).
        default_url_expiry_secs: u64,
    },
}

/// Errors returned when the `[storage]` config is invalid.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageBackendConfigError {
    /// `prod` profile selected `local` without
    /// `allow_local_in_production = true`.
    #[error(
        "storage.backend=local in the prod profile is unsafe across replicas; \
         set storage.allow_local_in_production=true to acknowledge"
    )]
    LocalInProduction,
    /// `s3` selected without a bucket.
    #[error("storage.backend=s3 requires storage.s3.bucket")]
    MissingS3Bucket,
    /// `s3` selected without a region.
    #[error("storage.backend=s3 requires storage.s3.region")]
    MissingS3Region,
    /// `s3` selected without the `storage-s3` cargo feature compiled in.
    #[error("storage.backend=s3 requires the `storage-s3` cargo feature")]
    S3FeatureDisabled,
}

impl StorageConfig {
    /// Resolve the concrete backend plan from config.
    ///
    /// # Errors
    /// Returns [`StorageBackendConfigError`] when the configured backend
    /// is missing required fields or unsafe for the active profile.
    pub fn backend_plan(
        &self,
        profile: Option<&str>,
    ) -> Result<StorageBackendPlan, StorageBackendConfigError> {
        match self.backend {
            StorageBackend::Disabled => Ok(StorageBackendPlan::Disabled),
            StorageBackend::Local => {
                if is_production_profile(profile) && !self.allow_local_in_production {
                    return Err(StorageBackendConfigError::LocalInProduction);
                }
                Ok(StorageBackendPlan::Local {
                    provider_id: self.default_provider.clone(),
                    root: self.local.root.clone(),
                    mount_path: self.local.mount_path.clone(),
                    default_url_expiry_secs: self.local.default_url_expiry_secs,
                    warn_in_production: is_production_profile(profile),
                })
            }
            StorageBackend::S3 => {
                let bucket = self
                    .s3
                    .bucket
                    .clone()
                    .filter(|b| !b.trim().is_empty())
                    .ok_or(StorageBackendConfigError::MissingS3Bucket)?;
                let region = self
                    .s3
                    .region
                    .clone()
                    .filter(|r| !r.trim().is_empty())
                    .ok_or(StorageBackendConfigError::MissingS3Region)?;

                #[cfg(feature = "storage-s3")]
                {
                    Ok(StorageBackendPlan::S3 {
                        provider_id: self.default_provider.clone(),
                        bucket,
                        region,
                        endpoint: self.s3.endpoint.clone(),
                        public_base_url: self.s3.public_base_url.clone(),
                        force_path_style: self.s3.force_path_style,
                        default_url_expiry_secs: self.s3.default_url_expiry_secs,
                    })
                }
                #[cfg(not(feature = "storage-s3"))]
                {
                    let _ = (bucket, region);
                    Err(StorageBackendConfigError::S3FeatureDisabled)
                }
            }
        }
    }
}

fn is_production_profile(profile: Option<&str>) -> bool {
    matches!(profile, Some("prod" | "production"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let cfg = StorageConfig::default();
        assert_eq!(cfg.backend, StorageBackend::Disabled);
        assert_eq!(cfg.default_provider, "default");
        assert_eq!(cfg.local.root, PathBuf::from("target/blobs"));
        assert_eq!(cfg.local.mount_path, "/_blobs");
    }

    #[test]
    fn backend_from_env_value() {
        assert_eq!(
            StorageBackend::from_env_value("local"),
            Some(StorageBackend::Local)
        );
        assert_eq!(
            StorageBackend::from_env_value("S3"),
            Some(StorageBackend::S3)
        );
        assert_eq!(
            StorageBackend::from_env_value("disabled"),
            Some(StorageBackend::Disabled)
        );
        assert_eq!(StorageBackend::from_env_value("memory"), None);
    }

    #[test]
    fn disabled_plan() {
        let cfg = StorageConfig::default();
        assert_eq!(
            cfg.backend_plan(Some("dev")),
            Ok(StorageBackendPlan::Disabled)
        );
    }

    #[test]
    fn local_plan_in_dev() {
        let cfg = StorageConfig {
            backend: StorageBackend::Local,
            ..Default::default()
        };
        let plan = cfg.backend_plan(Some("dev")).unwrap();
        match plan {
            StorageBackendPlan::Local {
                root,
                mount_path,
                warn_in_production,
                ..
            } => {
                assert_eq!(root, PathBuf::from("target/blobs"));
                assert_eq!(mount_path, "/_blobs");
                assert!(!warn_in_production);
            }
            other => panic!("expected Local plan, got {other:?}"),
        }
    }

    #[test]
    fn local_plan_rejects_prod_without_ack() {
        let cfg = StorageConfig {
            backend: StorageBackend::Local,
            ..Default::default()
        };
        assert_eq!(
            cfg.backend_plan(Some("prod")),
            Err(StorageBackendConfigError::LocalInProduction)
        );
    }

    #[test]
    fn local_plan_allows_prod_with_ack() {
        let cfg = StorageConfig {
            backend: StorageBackend::Local,
            allow_local_in_production: true,
            ..Default::default()
        };
        let plan = cfg.backend_plan(Some("prod")).unwrap();
        assert!(matches!(
            plan,
            StorageBackendPlan::Local {
                warn_in_production: true,
                ..
            }
        ));
    }

    #[test]
    fn s3_plan_requires_bucket_and_region() {
        let cfg = StorageConfig {
            backend: StorageBackend::S3,
            ..Default::default()
        };
        assert_eq!(
            cfg.backend_plan(Some("prod")),
            Err(StorageBackendConfigError::MissingS3Bucket)
        );

        let cfg = StorageConfig {
            backend: StorageBackend::S3,
            s3: StorageS3Config {
                bucket: Some("b".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            cfg.backend_plan(Some("prod")),
            Err(StorageBackendConfigError::MissingS3Region)
        );
    }

    #[test]
    #[cfg(not(feature = "storage-s3"))]
    fn s3_plan_yields_feature_disabled_error() {
        let cfg = StorageConfig {
            backend: StorageBackend::S3,
            s3: StorageS3Config {
                bucket: Some("b".into()),
                region: Some("r".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            cfg.backend_plan(Some("prod")),
            Err(StorageBackendConfigError::S3FeatureDisabled)
        );
    }
}
