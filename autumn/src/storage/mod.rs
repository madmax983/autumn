//! Pluggable file storage backends for Autumn applications.
//!
//! This module provides a [`BlobStore`] trait abstraction with one
//! built-in backend:
//!
//! - **[`Local`](local::LocalBlobStore)** — writes to a configurable root
//!   directory and serves bytes through an autumn-mounted route at
//!   `[storage.local].mount_path` (default `/_blobs`). URLs are signed
//!   with HMAC-SHA256 and time-bounded.
//!
//! For S3-compatible storage (AWS S3, Cloudflare R2, `MinIO`, `DigitalOcean`
//! Spaces, Wasabi) add the `autumn-storage-s3` crate and call
//! `.with_blob_store(S3BlobStore::from_config(&config.storage.s3).await?)`
//! on your [`AppBuilder`](crate::app::AppBuilder).
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
pub mod key;
pub mod migrations;

pub use blob::{Blob, BlobMeta};
pub use config::{
    StorageBackend, StorageBackendConfigError, StorageBackendPlan, StorageConfig,
    StorageLocalConfig, StorageS3Config,
};
pub use key::validate_key;
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
            // `Signature` is an auth failure (the URL was tampered with
            // or has expired), not a malformed-input error — map it
            // alongside `PermissionDenied` so handlers using `?` get
            // 403 consistently with what `local::serve_router` returns
            // directly when verifying a presigned URL.
            Self::PermissionDenied(_) | Self::Signature(_) => http::StatusCode::FORBIDDEN,
            Self::InvalidInput(_) => http::StatusCode::BAD_REQUEST,
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
/// Implement this trait to add new backends. The built-in backend is
/// [`LocalBlobStore`]. S3-compatible storage is provided by the
/// `autumn-storage-s3` crate.
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

