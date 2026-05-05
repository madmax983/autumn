//! S3-compatible [`BlobStore`](autumn_web::storage::BlobStore) plugin for autumn-web.
//!
//! # Quick start
//!
//! ```toml
//! [dependencies]
//! autumn-web     = { version = "0.3", features = ["storage", "multipart"] }
//! autumn-storage-s3 = "0.3"
//! ```
//!
//! ```rust,ignore
//! use autumn_storage_s3::S3BlobStore;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = autumn_web::config::TomlEnvConfigLoader::new()
//!         .load()
//!         .await
//!         .expect("failed to load config");
//!
//!     let store = S3BlobStore::from_config(&config.storage.s3)
//!         .await
//!         .expect("failed to build S3 store");
//!
//!     autumn_web::app()
//!         .with_blob_store(store)
//!         .run()
//!         .await;
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::Client;
use autumn_web::storage::{Blob, BlobFuture, BlobMeta, BlobStore, BlobStoreError, ByteStream, StorageS3Config};
use bytes::Bytes;
use futures::StreamExt as _;
use thiserror::Error;

/// Errors returned during [`S3BlobStore`] construction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum S3BlobStoreError {
    /// `[storage.s3].bucket` is not set.
    #[error("storage.s3.bucket is required but not set")]
    MissingBucket,
    /// `[storage.s3].region` is not set.
    #[error("storage.s3.region is required but not set")]
    MissingRegion,
    /// A credential env var listed in config is not present in the process
    /// environment.
    #[error("credential env var '{var}' is not set in the environment")]
    MissingCredentialEnvVar {
        /// Name of the missing environment variable.
        var: String,
    },
}

#[derive(Debug, Clone)]
struct S3Options {
    provider_id: String,
    bucket: String,
}

/// S3-compatible [`BlobStore`](autumn_web::storage::BlobStore) backed by
/// `aws-sdk-s3`.
///
/// Supports AWS S3, Cloudflare R2, `MinIO`, `DigitalOcean` Spaces, and Wasabi.
/// Construct with [`S3BlobStore::from_config`].
#[derive(Debug, Clone)]
pub struct S3BlobStore {
    client: Client,
    options: Arc<S3Options>,
}

impl S3BlobStore {
    /// Build an `S3BlobStore` from the `[storage.s3]` config section.
    ///
    /// Credentials resolve from `access_key_id_env` /
    /// `secret_access_key_env` when both are set; otherwise the AWS
    /// default credential chain (environment variables, instance metadata,
    /// IAM roles) is loaded.
    ///
    /// # Errors
    ///
    /// Returns [`S3BlobStoreError`] when required config fields are absent
    /// or a listed credential env var is missing from the environment.
    pub async fn from_config(cfg: &StorageS3Config) -> Result<Self, S3BlobStoreError> {
        let bucket = cfg
            .bucket
            .as_deref()
            .filter(|b| !b.trim().is_empty())
            .ok_or(S3BlobStoreError::MissingBucket)?
            .to_owned();
        let region = cfg
            .region
            .as_deref()
            .filter(|r| !r.trim().is_empty())
            .ok_or(S3BlobStoreError::MissingRegion)?
            .to_owned();

        let client = if let (Some(key_env), Some(secret_env)) =
            (&cfg.access_key_id_env, &cfg.secret_access_key_env)
        {
            let key = std::env::var(key_env).map_err(|_| S3BlobStoreError::MissingCredentialEnvVar {
                var: key_env.clone(),
            })?;
            let secret = std::env::var(secret_env).map_err(|_| S3BlobStoreError::MissingCredentialEnvVar {
                var: secret_env.clone(),
            })?;
            let creds = Credentials::new(key, secret, None, None, "autumn-storage-s3");
            let mut builder = aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new(region))
                .credentials_provider(creds)
                .force_path_style(cfg.force_path_style);
            if let Some(endpoint) = &cfg.endpoint {
                builder = builder.endpoint_url(endpoint);
            }
            Client::from_conf(builder.build())
        } else {
            let shared = aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(region))
                .load()
                .await;
            let mut builder = aws_sdk_s3::config::Builder::from(&shared)
                .force_path_style(cfg.force_path_style);
            if let Some(endpoint) = &cfg.endpoint {
                builder = builder.endpoint_url(endpoint);
            }
            Client::from_conf(builder.build())
        };

        Ok(Self {
            client,
            options: Arc::new(S3Options {
                provider_id: "s3".to_owned(),
                bucket,
            }),
        })
    }
}

impl BlobStore for S3BlobStore {
    fn provider_id(&self) -> &str {
        &self.options.provider_id
    }

    fn put<'a>(&'a self, key: &'a str, content_type: &'a str, bytes: Bytes) -> BlobFuture<'a, Blob> {
        let byte_size = bytes.len() as u64;
        Box::pin(async move {
            let result = self
                .client
                .put_object()
                .bucket(&self.options.bucket)
                .key(key)
                .content_type(content_type)
                .body(bytes.into())
                .send()
                .await
                .map_err(|e| BlobStoreError::backend(e.to_string()))?;
            let etag = result.e_tag().map(str::to_owned);
            let mut blob = Blob::new(&self.options.provider_id, key, content_type, byte_size);
            if let Some(e) = etag {
                blob = blob.with_etag(e);
            }
            Ok(blob)
        })
    }

    fn put_stream<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        data: ByteStream<'a>,
    ) -> BlobFuture<'a, Blob> {
        Box::pin(async move {
            // Collect the stream into memory then delegate to put.
            // A true multipart upload would be better for large files,
            // but this keeps the implementation simple and correct for
            // the common case.
            let mut buf = Vec::new();
            let mut stream = data;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                buf.extend_from_slice(&chunk);
            }
            self.put(key, content_type, Bytes::from(buf)).await
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Bytes> {
        Box::pin(async move {
            let result = self
                .client
                .get_object()
                .bucket(&self.options.bucket)
                .key(key)
                .send()
                .await
                .map_err(|e| {
                    let msg = e.to_string();
                    if msg.contains("NoSuchKey") || msg.contains("404") {
                        BlobStoreError::NotFound(key.to_owned())
                    } else {
                        BlobStoreError::backend(msg)
                    }
                })?;
            let body = result
                .body
                .collect()
                .await
                .map_err(|e| BlobStoreError::io(e.to_string()))?;
            Ok(body.into_bytes())
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BlobFuture<'a, ()> {
        Box::pin(async move {
            self.client
                .delete_object()
                .bucket(&self.options.bucket)
                .key(key)
                .send()
                .await
                .map_err(|e| BlobStoreError::backend(e.to_string()))?;
            Ok(())
        })
    }

    fn head<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Option<BlobMeta>> {
        Box::pin(async move {
            let result = self
                .client
                .head_object()
                .bucket(&self.options.bucket)
                .key(key)
                .send()
                .await;
            match result {
                Ok(resp) => {
                    let content_type = resp
                        .content_type()
                        .unwrap_or("application/octet-stream")
                        .to_owned();
                    let byte_size = resp
                        .content_length()
                        .and_then(|n| u64::try_from(n).ok())
                        .unwrap_or(0);
                    let etag = resp.e_tag().map(str::to_owned);
                    Ok(Some(BlobMeta {
                        key: key.to_owned(),
                        content_type,
                        byte_size,
                        etag,
                    }))
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("NoSuchKey") || msg.contains("404") || msg.contains("NotFound") {
                        Ok(None)
                    } else {
                        Err(BlobStoreError::backend(msg))
                    }
                }
            }
        })
    }

    fn presigned_url<'a>(&'a self, key: &'a str, expires_in: Duration) -> BlobFuture<'a, String> {
        Box::pin(async move {
            let presigning = PresigningConfig::expires_in(expires_in)
                .map_err(|e| BlobStoreError::backend(e.to_string()))?;
            let req = self
                .client
                .get_object()
                .bucket(&self.options.bucket)
                .key(key)
                .presigned(presigning)
                .await
                .map_err(|e| BlobStoreError::backend(e.to_string()))?;
            Ok(req.uri().to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_config(bucket: Option<&str>, region: Option<&str>) -> StorageS3Config {
        StorageS3Config {
            bucket: bucket.map(str::to_owned),
            region: region.map(str::to_owned),
            ..StorageS3Config::default()
        }
    }

    #[tokio::test]
    async fn from_config_requires_bucket() {
        let cfg = s3_config(None, Some("us-east-1"));
        let err = S3BlobStore::from_config(&cfg).await.unwrap_err();
        assert!(matches!(err, S3BlobStoreError::MissingBucket));
    }

    #[tokio::test]
    async fn from_config_requires_region() {
        let cfg = s3_config(Some("my-bucket"), None);
        let err = S3BlobStore::from_config(&cfg).await.unwrap_err();
        assert!(matches!(err, S3BlobStoreError::MissingRegion));
    }

    #[tokio::test]
    async fn from_config_empty_bucket_is_rejected() {
        let cfg = s3_config(Some("   "), Some("us-east-1"));
        let err = S3BlobStore::from_config(&cfg).await.unwrap_err();
        assert!(matches!(err, S3BlobStoreError::MissingBucket));
    }

    #[tokio::test]
    async fn from_config_empty_region_is_rejected() {
        let cfg = s3_config(Some("my-bucket"), Some(""));
        let err = S3BlobStore::from_config(&cfg).await.unwrap_err();
        assert!(matches!(err, S3BlobStoreError::MissingRegion));
    }

    #[tokio::test]
    async fn from_config_missing_credential_env_var() {
        let cfg = StorageS3Config {
            bucket: Some("b".into()),
            region: Some("us-east-1".into()),
            access_key_id_env: Some("__AUTUMN_TEST_KEY_ID_DEFINITELY_NOT_SET__".into()),
            secret_access_key_env: Some("__AUTUMN_TEST_SECRET_DEFINITELY_NOT_SET__".into()),
            ..StorageS3Config::default()
        };
        let err = S3BlobStore::from_config(&cfg).await.unwrap_err();
        assert!(matches!(err, S3BlobStoreError::MissingCredentialEnvVar { .. }));
    }

    #[tokio::test]
    async fn from_config_with_static_creds_sets_provider_id() {
        let cfg = StorageS3Config {
            bucket: Some("test-bucket".into()),
            region: Some("us-east-1".into()),
            access_key_id_env: Some("__AUTUMN_S3_TEST_KEY_ID__".into()),
            secret_access_key_env: Some("__AUTUMN_S3_TEST_SECRET__".into()),
            ..StorageS3Config::default()
        };
        Box::pin(temp_env::async_with_vars(
            [
                ("__AUTUMN_S3_TEST_KEY_ID__", Some("AKIAIOSFODNN7EXAMPLE")),
                ("__AUTUMN_S3_TEST_SECRET__", Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")),
            ],
            async {
                let store = S3BlobStore::from_config(&cfg).await.expect("should build with static creds");
                assert_eq!(store.provider_id(), "s3");
            },
        ))
        .await;
    }

    #[tokio::test]
    async fn from_config_with_custom_endpoint() {
        let cfg = StorageS3Config {
            bucket: Some("my-bucket".into()),
            region: Some("auto".into()),
            endpoint: Some("https://example.r2.cloudflarestorage.com".into()),
            force_path_style: true,
            access_key_id_env: Some("__AUTUMN_S3_TEST_KEY_ID2__".into()),
            secret_access_key_env: Some("__AUTUMN_S3_TEST_SECRET2__".into()),
            ..StorageS3Config::default()
        };
        Box::pin(temp_env::async_with_vars(
            [
                ("__AUTUMN_S3_TEST_KEY_ID2__", Some("key")),
                ("__AUTUMN_S3_TEST_SECRET2__", Some("secret")),
            ],
            async {
                let store = S3BlobStore::from_config(&cfg).await.expect("should build");
                assert_eq!(store.provider_id(), "s3");
            },
        ))
        .await;
    }

    #[test]
    fn implements_blob_store_trait() {
        fn assert_impl<T: BlobStore>() {}
        assert_impl::<S3BlobStore>();
    }

    #[test]
    fn implements_send_sync_clone() {
        fn assert_impl<T: Send + Sync + Clone>() {}
        assert_impl::<S3BlobStore>();
    }
}
