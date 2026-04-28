//! Pluggable file storage backends for Autumn applications.
//!
//! This module provides a [`BlobStore`] trait abstraction with two
//! built-in backends:
//!
//! - **[`Local`](local::LocalBlobStore)** — writes to a configurable root
//!   directory and serves bytes through an autumn-mounted route at
//!   `[storage.local].mount_path` (default `/_blobs`). URLs are signed
//!   with HMAC-SHA256 and time-bounded.
//! - **[`S3`](s3::S3BlobStore)** (gated behind `storage-s3`) — talks to
//!   any S3-compatible endpoint (AWS S3, Cloudflare R2, `MinIO`,
//!   `DigitalOcean` Spaces, Wasabi) and emits real S3 presigned URLs.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use autumn_web::storage::{Blob, BlobStore, BlobStoreError};
//!
//! async fn upload<S: BlobStore + ?Sized>(store: &S, key: &str, bytes: bytes::Bytes)
//!     -> Result<Blob, BlobStoreError>
//! {
//!     store.put(key, "image/png", bytes).await
//! }
//! ```
//!
//! ## Profile-aware defaults
//!
//! | Profile | Default backend | Notes |
//! |---------|-----------------|-------|
//! | `dev`   | `Local` rooted at `target/blobs/` | Always-on with the `storage` feature |
//! | `prod`  | Fail-fast on `local` unless `storage.allow_local_in_production = true` | Force explicit acknowledgement of multi-replica risk |
//!
//! ## Configuration
//!
//! ```toml
//! [storage]
//! backend = "local"        # "local" | "s3" | "disabled"
//! default_provider = "default"
//!
//! [storage.local]
//! root = "target/blobs"
//! mount_path = "/_blobs"
//!
//! [storage.s3]
//! bucket = "my-app-uploads"
//! region = "us-east-1"
//! endpoint = "https://s3.amazonaws.com"
//! access_key_id_env = "AWS_ACCESS_KEY_ID"
//! secret_access_key_env = "AWS_SECRET_ACCESS_KEY"
//! force_path_style = false
//! ```

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use thiserror::Error;

pub mod blob;
pub mod config;
pub mod local;
pub mod migrations;
#[cfg(feature = "storage-s3")]
pub mod s3;

pub use blob::{Blob, BlobMeta};
pub use config::{
    StorageBackend, StorageBackendConfigError, StorageBackendPlan, StorageConfig,
    StorageLocalConfig, StorageS3Config,
};
pub use local::LocalBlobStore;

/// Boxed future returned by [`BlobStore`] methods.
///
/// Pinning the future as a trait object keeps [`BlobStore`] dyn-safe so
/// applications can hold an `Arc<dyn BlobStore>` and swap backends at
/// runtime.
pub type BlobFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BlobStoreError>> + Send + 'a>>;

/// Stream of byte chunks accepted by [`BlobStore::put_stream`].
///
/// Each item is a `bytes::Bytes` chunk; errors propagate as
/// [`BlobStoreError`]. The lifetime parameter lets callers borrow the
/// chunk source from their own stack (e.g. an in-flight multipart
/// extractor) without forcing a `'static` bound.
pub type ByteStream<'a> = Pin<Box<dyn Stream<Item = Result<Bytes, BlobStoreError>> + Send + 'a>>;

/// Errors returned by [`BlobStore`] operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BlobStoreError {
    /// The requested key was not found.
    #[error("blob not found: {0}")]
    NotFound(String),

    /// Authentication or authorization failed against the backend.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Invalid input — most often a malformed or unsafe key.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Caller exceeded a backend size limit (per-file or per-request).
    /// Maps to HTTP `413 Payload Too Large`.
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    /// I/O failure (filesystem, network, transport).
    #[error("io error: {0}")]
    Io(String),

    /// The configured backend doesn't support this operation.
    #[error("operation not supported: {0}")]
    Unsupported(String),

    /// A signed URL could not be verified or has expired.
    #[error("signature error: {0}")]
    Signature(String),

    /// Backend-specific error reported as a string for portability.
    #[error("backend error: {0}")]
    Backend(String),
}

impl BlobStoreError {
    /// Wrap an `io::Error` for the I/O variant.
    #[must_use]
    pub fn io(err: impl std::fmt::Display) -> Self {
        Self::Io(err.to_string())
    }

    /// Convenience constructor for [`BlobStoreError::Backend`].
    #[must_use]
    pub fn backend(err: impl std::fmt::Display) -> Self {
        Self::Backend(err.to_string())
    }
}

impl BlobStoreError {
    /// HTTP status code that best fits this error variant.
    #[must_use]
    pub const fn status(&self) -> http::StatusCode {
        match self {
            Self::NotFound(_) => http::StatusCode::NOT_FOUND,
            Self::PermissionDenied(_) => http::StatusCode::FORBIDDEN,
            Self::InvalidInput(_) | Self::Signature(_) => http::StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge(_) => http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::Unsupported(_) => http::StatusCode::NOT_IMPLEMENTED,
            Self::Io(_) | Self::Backend(_) => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Promote into an [`AutumnError`](crate::AutumnError) carrying the
    /// status from [`BlobStoreError::status`].
    ///
    /// Handlers using `?` get a 500 by default via the blanket
    /// `impl From<E: Error> for AutumnError`; call this instead to
    /// preserve the precise status (404 for missing blobs, 403 for
    /// signature failures, etc.).
    #[must_use]
    pub fn into_autumn_error(self) -> crate::AutumnError {
        let status = self.status();
        crate::AutumnError::internal_server_error(self).with_status(status)
    }
}

/// Pluggable file-storage backend.
///
/// Implement this trait to add new backends. Two are provided
/// out of the box: [`LocalBlobStore`] and (with feature `storage-s3`)
/// [`s3::S3BlobStore`].
///
/// The trait is **dyn-safe** so apps can hold `Arc<dyn BlobStore>` and
/// swap backends at runtime — for example, choosing local in tests and
/// S3 in production via configuration.
pub trait BlobStore: Send + Sync + 'static {
    /// Stable identifier for the configured provider, recorded on every
    /// [`Blob`] so applications can detect cross-store mismatches.
    fn provider_id(&self) -> &str;

    /// Store `bytes` under `key`, returning a [`Blob`] handle.
    fn put<'a>(&'a self, key: &'a str, content_type: &'a str, bytes: Bytes)
    -> BlobFuture<'a, Blob>;

    /// Stream `data` under `key`, returning a [`Blob`] handle.
    ///
    /// Use this for files larger than memory.
    fn put_stream<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        data: ByteStream<'a>,
    ) -> BlobFuture<'a, Blob>;

    /// Read the bytes for `key` into memory.
    fn get<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Bytes>;

    /// Delete the blob at `key`. No-op when the key does not exist.
    fn delete<'a>(&'a self, key: &'a str) -> BlobFuture<'a, ()>;

    /// Return metadata for `key` if it exists.
    fn head<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Option<BlobMeta>>;

    /// Build a time-bounded URL that serves the blob's bytes.
    ///
    /// On the [`LocalBlobStore`] this is an HMAC-signed link to the
    /// mounted serving route. On S3 backends it is a real S3 presigned
    /// URL.
    fn presigned_url<'a>(&'a self, key: &'a str, expires_in: Duration) -> BlobFuture<'a, String>;
}

/// Type alias for a runtime-installed shared [`BlobStore`].
///
/// Applications obtain this from [`AppState`](crate::AppState) via
/// [`BlobStoreState::store`].
pub type SharedBlobStore = Arc<dyn BlobStore>;

/// Wrapper installed on [`AppState`](crate::AppState) so handlers can
/// pull the configured store back out via
/// [`AppState::extension::<BlobStoreState>()`](crate::AppState::extension).
#[derive(Clone)]
pub struct BlobStoreState {
    inner: SharedBlobStore,
}

impl BlobStoreState {
    /// Wrap a runtime-built blob store.
    #[must_use]
    pub fn new(store: SharedBlobStore) -> Self {
        Self { inner: store }
    }

    /// Borrow the underlying [`BlobStore`] handle.
    #[must_use]
    pub fn store(&self) -> &SharedBlobStore {
        &self.inner
    }
}

/// Validate a user-supplied object key.
///
/// Keys must be non-empty, must not contain `..` segments, must not be
/// absolute paths, and must not contain NUL bytes. This protects the
/// [`LocalBlobStore`] from path-traversal and gives S3 backends a
/// consistent input contract.
///
/// # Errors
///
/// Returns [`BlobStoreError::InvalidInput`] when the key is rejected.
pub fn validate_key(key: &str) -> Result<(), BlobStoreError> {
    if key.is_empty() {
        return Err(BlobStoreError::InvalidInput("blob key is empty".into()));
    }
    if key.contains('\0') {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains NUL byte".into(),
        ));
    }
    if key.starts_with('/') || key.starts_with('\\') {
        return Err(BlobStoreError::InvalidInput(
            "blob key must be relative".into(),
        ));
    }
    // Windows drive-letter forms (`C:\…`, `C:/…`, `\\?\…`, `\\server\share\…`)
    // would be treated as absolute by `Path::join` on Windows, silently
    // escaping the storage root regardless of whether the host happens
    // to be Linux. Reject them up-front so the same key contract holds
    // on every platform.
    let bytes = key.as_bytes();
    let drive_letter = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive_letter {
        return Err(BlobStoreError::InvalidInput(
            "blob key looks like a Windows drive-letter path".into(),
        ));
    }
    if key.starts_with("\\\\") || key.starts_with("//") {
        return Err(BlobStoreError::InvalidInput(
            "blob key looks like a UNC / network path".into(),
        ));
    }
    for segment in key.split(['/', '\\']) {
        if segment == ".." {
            return Err(BlobStoreError::InvalidInput(
                "blob key contains traversal segment".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_accepts_typical_paths() {
        validate_key("avatars/123.png").unwrap();
        validate_key("a/b/c/d.txt").unwrap();
    }

    #[test]
    fn validate_key_rejects_traversal() {
        let err = validate_key("../etc/passwd").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_absolute() {
        let err = validate_key("/etc/passwd").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_empty() {
        let err = validate_key("").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_nul() {
        let err = validate_key("a\0b").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_windows_drive_letter() {
        for k in [r"C:\tmp\x", "C:/tmp/x", "z:\\foo", "a:bar"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_unc_paths() {
        for k in [r"\\server\share\file", "//server/share/file"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn error_status_mapping() {
        assert_eq!(
            BlobStoreError::NotFound("x".into()).status(),
            http::StatusCode::NOT_FOUND
        );
        assert_eq!(
            BlobStoreError::PermissionDenied("x".into()).status(),
            http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            BlobStoreError::InvalidInput("x".into()).status(),
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            BlobStoreError::Signature("x".into()).status(),
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            BlobStoreError::PayloadTooLarge("x".into()).status(),
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            BlobStoreError::Unsupported("x".into()).status(),
            http::StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            BlobStoreError::Backend("x".into()).status(),
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn error_into_autumn_error_preserves_status() {
        let err = BlobStoreError::NotFound("k".into()).into_autumn_error();
        assert_eq!(err.status(), http::StatusCode::NOT_FOUND);
    }
}
