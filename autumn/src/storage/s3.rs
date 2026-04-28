//! S3-compatible implementation of [`BlobStore`](super::BlobStore).
//!
//! This module is gated behind the `storage-s3` cargo feature.
//!
//! # Status
//!
//! The trait surface, configuration, and presigned-URL plumbing live
//! in this module, but the on-the-wire S3 client is **not yet wired
//! in**. Operations return [`BlobStoreError::Unsupported`] at runtime
//! so applications fail loudly rather than silently dropping bytes.
//!
//! Hooking up an SDK (engineering's call between `aws-sdk-s3` and
//! `rust-s3`) is tracked as a follow-up in
//! [`docs/guide/storage.md`](../../../docs/guide/storage.md). The trait
//! surface and config story are designed to be SDK-agnostic so the swap
//! is local to this file.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use super::blob::{Blob, BlobMeta};
use super::{BlobFuture, BlobStore, BlobStoreError, ByteStream};

/// Options assembled from `[storage.s3]`.
#[derive(Debug, Clone)]
pub struct S3Options {
    /// Stable provider id recorded on every [`Blob`].
    pub provider_id: String,
    /// Target bucket.
    pub bucket: String,
    /// Region (or region-shaped string for non-AWS providers).
    pub region: String,
    /// Custom endpoint URL (R2, `MinIO`, DO Spaces, Wasabi).
    pub endpoint: Option<String>,
    /// Public base URL for presigned URLs.
    pub public_base_url: Option<String>,
    /// Path-style addressing.
    pub force_path_style: bool,
    /// Default presigned URL expiry.
    pub default_expiry: Duration,
}

/// S3-compatible blob store.
#[derive(Clone)]
pub struct S3BlobStore {
    inner: Arc<S3Options>,
}

impl S3BlobStore {
    /// Build a store from already-validated [`S3Options`].
    #[must_use]
    pub fn new(options: S3Options) -> Self {
        Self {
            inner: Arc::new(options),
        }
    }

    /// Borrow the configured options.
    #[must_use]
    pub fn options(&self) -> &S3Options {
        &self.inner
    }
}

impl BlobStore for S3BlobStore {
    fn provider_id(&self) -> &str {
        &self.inner.provider_id
    }

    fn put<'a>(
        &'a self,
        _key: &'a str,
        _content_type: &'a str,
        _bytes: Bytes,
    ) -> BlobFuture<'a, Blob> {
        Box::pin(async move { Err(unsupported("S3BlobStore::put")) })
    }

    fn put_stream<'a>(
        &'a self,
        _key: &'a str,
        _content_type: &'a str,
        _data: ByteStream<'a>,
    ) -> BlobFuture<'a, Blob> {
        Box::pin(async move { Err(unsupported("S3BlobStore::put_stream")) })
    }

    fn get<'a>(&'a self, _key: &'a str) -> BlobFuture<'a, Bytes> {
        Box::pin(async move { Err(unsupported("S3BlobStore::get")) })
    }

    fn delete<'a>(&'a self, _key: &'a str) -> BlobFuture<'a, ()> {
        Box::pin(async move { Err(unsupported("S3BlobStore::delete")) })
    }

    fn head<'a>(&'a self, _key: &'a str) -> BlobFuture<'a, Option<BlobMeta>> {
        Box::pin(async move { Err(unsupported("S3BlobStore::head")) })
    }

    fn presigned_url<'a>(&'a self, _key: &'a str, _expires_in: Duration) -> BlobFuture<'a, String> {
        Box::pin(async move { Err(unsupported("S3BlobStore::presigned_url")) })
    }
}

fn unsupported(op: &str) -> BlobStoreError {
    BlobStoreError::Unsupported(format!(
        "{op} is not yet implemented; pin an SDK in storage::s3 to enable"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;

    fn store() -> S3BlobStore {
        S3BlobStore::new(S3Options {
            provider_id: "s3-prod".into(),
            bucket: "uploads".into(),
            region: "us-east-1".into(),
            endpoint: Some("https://example.com".into()),
            public_base_url: None,
            force_path_style: false,
            default_expiry: Duration::from_secs(60),
        })
    }

    #[test]
    fn options_round_trip() {
        let s = store();
        assert_eq!(s.provider_id(), "s3-prod");
        assert_eq!(s.options().bucket, "uploads");
        assert_eq!(s.options().region, "us-east-1");
    }

    #[tokio::test]
    async fn put_returns_unsupported() {
        let err = store()
            .put("k", "image/png", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[tokio::test]
    async fn put_stream_returns_unsupported() {
        let chunks: Vec<Result<Bytes, BlobStoreError>> = vec![Ok(Bytes::from_static(b"x"))];
        let stream: ByteStream<'static> = Box::pin(stream::iter(chunks));
        let err = store()
            .put_stream("k", "image/png", stream)
            .await
            .unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[tokio::test]
    async fn get_returns_unsupported() {
        let err = store().get("k").await.unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[tokio::test]
    async fn delete_returns_unsupported() {
        let err = store().delete("k").await.unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[tokio::test]
    async fn head_returns_unsupported() {
        let err = store().head("k").await.unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[tokio::test]
    async fn presigned_url_returns_unsupported() {
        let err = store()
            .presigned_url("k", Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(matches!(err, BlobStoreError::Unsupported(_)));
    }

    #[test]
    fn unsupported_message_includes_operation_and_hint() {
        let err = unsupported("S3BlobStore::put");
        let msg = err.to_string();
        assert!(msg.contains("S3BlobStore::put"));
        assert!(msg.contains("storage::s3"));
    }
}
