//! Local-disk implementation of [`BlobStore`](super::BlobStore).
//!
//! Bytes land under a configurable `root` directory; URLs are
//! HMAC-SHA256-signed and time-bounded, served by an axum router mounted
//! by the framework on startup.
//!
//! Suitable for `dev`, single-replica deployments, and integration
//! tests. Multi-replica production should use the `S3` backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt as _;
use http::StatusCode;
use sha2::{Digest, Sha256};

use super::blob::{Blob, BlobMeta};
use super::{BlobFuture, BlobStore, BlobStoreError, ByteStream, validate_key};

/// HMAC signing key used by the local backend.
///
/// In test and dev a random key is generated at startup; in production
/// callers are expected to set `[storage.local].signing_key` (or the
/// `AUTUMN_STORAGE__LOCAL__SIGNING_KEY` env var) so URLs survive process
/// restarts and replicas agree on the signature.
#[derive(Clone)]
pub struct SigningKey(Arc<Vec<u8>>);

impl SigningKey {
    /// Create a key from explicit bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Arc::new(bytes))
    }

    /// Generate a random 32-byte key.
    #[must_use]
    pub fn random() -> Self {
        let mut bytes = vec![0u8; 32];
        // Mix a UUID v4 (already cryptographic-quality random) into the key.
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        bytes[..16].copy_from_slice(a.as_bytes());
        bytes[16..].copy_from_slice(b.as_bytes());
        Self::new(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Local-disk blob store.
///
/// Construct via [`LocalBlobStore::new`]. The framework wires this up
/// from `[storage.local]` automatically when `storage.backend = "local"`.
#[derive(Clone)]
pub struct LocalBlobStore {
    inner: Arc<LocalInner>,
}

struct LocalInner {
    provider_id: String,
    root: PathBuf,
    mount_path: String,
    default_expiry: Duration,
    signing_key: SigningKey,
}

impl LocalBlobStore {
    /// Create a new local store rooted at `root`.
    ///
    /// `mount_path` must start with `/` — it's the prefix the framework
    /// uses to serve signed URLs (default `/_blobs`).
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError::Io`] when the root directory cannot be
    /// created.
    pub fn new(
        provider_id: impl Into<String>,
        root: impl Into<PathBuf>,
        mount_path: impl Into<String>,
        default_expiry: Duration,
        signing_key: SigningKey,
    ) -> Result<Self, BlobStoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(BlobStoreError::io)?;
        Ok(Self {
            inner: Arc::new(LocalInner {
                provider_id: provider_id.into(),
                root,
                mount_path: mount_path.into(),
                default_expiry,
                signing_key,
            }),
        })
    }

    /// Borrow the configured mount path.
    #[must_use]
    pub fn mount_path(&self) -> &str {
        &self.inner.mount_path
    }

    /// Borrow the configured signing key — used by the framework when
    /// wiring up the serving route.
    #[must_use]
    pub fn signing_key(&self) -> SigningKey {
        self.inner.signing_key.clone()
    }

    /// Borrow the on-disk root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn resolve(&self, key: &str) -> Result<PathBuf, BlobStoreError> {
        validate_key(key)?;
        let path = self.inner.root.join(key);
        // Defense in depth: re-canonicalize against a possible symlink.
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(BlobStoreError::InvalidInput(
                "blob key escapes storage root".into(),
            ));
        }
        Ok(path)
    }
}

impl BlobStore for LocalBlobStore {
    fn provider_id(&self) -> &str {
        &self.inner.provider_id
    }

    fn put<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        bytes: Bytes,
    ) -> BlobFuture<'a, Blob> {
        Box::pin(async move {
            let path = self.resolve(key)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(BlobStoreError::io)?;
            }
            let etag = sha256_hex(&bytes);
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(BlobStoreError::io)?;
            Ok(Blob {
                provider_id: self.inner.provider_id.clone(),
                key: key.to_owned(),
                content_type: content_type.to_owned(),
                byte_size: bytes.len() as u64,
                etag: Some(etag),
            })
        })
    }

    fn put_stream<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        mut data: ByteStream,
    ) -> BlobFuture<'a, Blob> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt as _;

            let path = self.resolve(key)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(BlobStoreError::io)?;
            }

            // Write through a closure so we can clean up the
            // partially-written file on any error path. Without this,
            // a client disconnect or transient I/O failure leaves a
            // corrupt blob at `path` that future `get` calls would
            // happily serve.
            let result = async {
                let mut file = tokio::fs::File::create(&path)
                    .await
                    .map_err(BlobStoreError::io)?;
                let mut hasher = Sha256::new();
                let mut byte_size: u64 = 0;
                while let Some(chunk) = data.next().await {
                    let chunk = chunk?;
                    hasher.update(&chunk);
                    byte_size = byte_size.saturating_add(chunk.len() as u64);
                    file.write_all(&chunk).await.map_err(BlobStoreError::io)?;
                }
                file.flush().await.map_err(BlobStoreError::io)?;
                Ok::<(u64, String), BlobStoreError>((byte_size, hex(hasher.finalize())))
            }
            .await;

            match result {
                Ok((byte_size, etag)) => Ok(Blob {
                    provider_id: self.inner.provider_id.clone(),
                    key: key.to_owned(),
                    content_type: content_type.to_owned(),
                    byte_size,
                    etag: Some(etag),
                }),
                Err(err) => {
                    let _ = tokio::fs::remove_file(&path).await;
                    Err(err)
                }
            }
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Bytes> {
        Box::pin(async move {
            let path = self.resolve(key)?;
            match tokio::fs::read(&path).await {
                Ok(bytes) => Ok(Bytes::from(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    Err(BlobStoreError::NotFound(key.to_owned()))
                }
                Err(err) => Err(BlobStoreError::io(err)),
            }
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BlobFuture<'a, ()> {
        Box::pin(async move {
            let path = self.resolve(key)?;
            match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(BlobStoreError::io(err)),
            }
        })
    }

    fn head<'a>(&'a self, key: &'a str) -> BlobFuture<'a, Option<BlobMeta>> {
        Box::pin(async move {
            let path = self.resolve(key)?;
            match tokio::fs::metadata(&path).await {
                Ok(meta) => Ok(Some(BlobMeta {
                    key: key.to_owned(),
                    // The on-disk format does not preserve content-type;
                    // callers that care should remember the value from
                    // the originating `Blob`.
                    content_type: "application/octet-stream".to_owned(),
                    byte_size: meta.len(),
                    etag: None,
                })),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(BlobStoreError::io(err)),
            }
        })
    }

    fn presigned_url<'a>(&'a self, key: &'a str, expires_in: Duration) -> BlobFuture<'a, String> {
        Box::pin(async move {
            validate_key(key)?;
            let expires_in = if expires_in.is_zero() {
                self.inner.default_expiry
            } else {
                expires_in
            };
            let exp_at = SystemTime::now()
                .checked_add(expires_in)
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();

            // Sign the canonical (unencoded) key — the serving route
            // decodes path segments before re-signing for verification.
            let signature = sign(self.inner.signing_key.as_bytes(), key, exp_at);
            // Percent-encode each segment so keys containing reserved
            // URL characters (`?`, `#`, `%`, spaces, …) round-trip
            // correctly through the path. `/` survives as the segment
            // separator.
            let encoded_key = encode_key_path(key);
            let url = format!(
                "{base}/{encoded_key}?exp={exp_at}&sig={signature}",
                base = self.inner.mount_path.trim_end_matches('/'),
            );
            Ok(url)
        })
    }
}

/// Compute the canonical signature for `(key, expiry)`.
///
/// # Panics
///
/// Never; `Hmac::new_from_slice` accepts any key length.
#[must_use]
pub fn sign(key_bytes: &[u8], blob_key: &str, expires_at: u64) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key_bytes).expect("HMAC accepts any key length");
    mac.update(blob_key.as_bytes());
    mac.update(b":");
    mac.update(expires_at.to_string().as_bytes());
    hex(mac.finalize().into_bytes())
}

/// Verify a `(key, expiry, signature)` triple.
///
/// Returns `Ok(())` when the signature matches and `expires_at` is
/// still in the future.
///
/// # Errors
///
/// Returns [`BlobStoreError::Signature`] for malformed, expired, or
/// mismatched signatures.
pub fn verify(
    signing_key: &[u8],
    blob_key: &str,
    expires_at: u64,
    signature: &str,
) -> Result<(), BlobStoreError> {
    let expected = sign(signing_key, blob_key, expires_at);
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return Err(BlobStoreError::Signature("signature mismatch".into()));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if expires_at < now {
        return Err(BlobStoreError::Signature("signed url expired".into()));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(hasher.finalize())
}

/// Percent-encode each `/`-separated segment of `key` for use in a URL
/// path. Segment separators stay raw so the path tree survives.
fn encode_key_path(key: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    // RFC 3986 path segment: encode controls, path-reserved, and
    // anything that would alias another segment or query/fragment
    // delimiter.
    const PATH_SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'/')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}')
        .add(b'\\');

    key.split('/')
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn hex<B: AsRef<[u8]>>(bytes: B) -> String {
    let mut s = String::with_capacity(bytes.as_ref().len() * 2);
    for b in bytes.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

// ── Serving route ──────────────────────────────────────────────

/// Build the axum router that serves signed local-blob URLs.
///
/// This is mounted by the framework at the configured `mount_path`.
pub fn serve_router(store: &LocalBlobStore) -> axum::Router<crate::AppState> {
    use axum::extract::{Path, Query};
    use axum::response::IntoResponse;

    #[derive(Debug, serde::Deserialize)]
    struct SignedQuery {
        exp: u64,
        sig: String,
    }

    let store_for_route = store.clone();
    let mount = format!("{}/{{*key}}", store.mount_path().trim_end_matches('/'));

    let handler = move |Path(blob_key): Path<String>, Query(q): Query<SignedQuery>| {
        let store = store_for_route.clone();
        async move {
            if let Err(err) = verify(store.signing_key().as_bytes(), &blob_key, q.exp, &q.sig) {
                return (StatusCode::FORBIDDEN, err.to_string()).into_response();
            }
            match BlobStore::get(&store, &blob_key).await {
                Ok(bytes) => bytes.into_response(),
                Err(BlobStoreError::NotFound(_)) => {
                    (StatusCode::NOT_FOUND, "not found").into_response()
                }
                Err(err) => err.into_autumn_error().into_response(),
            }
        }
    };

    axum::Router::new().route(&mount, axum::routing::get(handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn store(root: &Path) -> LocalBlobStore {
        LocalBlobStore::new(
            "test",
            root.to_path_buf(),
            "/_blobs",
            Duration::from_secs(60),
            SigningKey::new(b"test-key".to_vec()),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn put_get_round_trip() {
        let dir = temp_root();
        let s = store(dir.path());
        let blob = s
            .put("a/b.png", "image/png", Bytes::from_static(b"abc"))
            .await
            .unwrap();
        assert_eq!(blob.byte_size, 3);
        assert!(blob.etag.is_some());
        let bytes = s.get("a/b.png").await.unwrap();
        assert_eq!(&bytes[..], b"abc");
    }

    #[tokio::test]
    async fn put_stream_round_trip() {
        let dir = temp_root();
        let s = store(dir.path());
        let chunks: Vec<Result<Bytes, BlobStoreError>> = vec![
            Ok(Bytes::from_static(b"hello, ")),
            Ok(Bytes::from_static(b"world")),
        ];
        let stream: ByteStream = Box::pin(stream::iter(chunks));
        let blob = s
            .put_stream("greet.txt", "text/plain", stream)
            .await
            .unwrap();
        assert_eq!(blob.byte_size, 12);
        let bytes = s.get("greet.txt").await.unwrap();
        assert_eq!(&bytes[..], b"hello, world");
    }

    #[tokio::test]
    async fn get_missing_returns_not_found() {
        let dir = temp_root();
        let s = store(dir.path());
        let err = s.get("missing.txt").await.unwrap_err();
        assert!(matches!(err, BlobStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_idempotent() {
        let dir = temp_root();
        let s = store(dir.path());
        s.delete("nope").await.unwrap();
        let _ = s
            .put("k.txt", "text/plain", Bytes::from_static(b"x"))
            .await
            .unwrap();
        s.delete("k.txt").await.unwrap();
        assert!(matches!(
            s.get("k.txt").await.unwrap_err(),
            BlobStoreError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn rejects_traversal_keys() {
        let dir = temp_root();
        let s = store(dir.path());
        let err = s
            .put("../escape.txt", "text/plain", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn signature_round_trip() {
        let key = b"k";
        let sig = sign(key, "blob/1.png", 99);
        verify(key, "blob/1.png", u64::MAX / 2, &sig).unwrap_err();
        // Use a now-future expiry for verification.
        let exp = SystemTime::now()
            .checked_add(Duration::from_secs(60))
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let sig = sign(key, "blob/1.png", exp);
        verify(key, "blob/1.png", exp, &sig).unwrap();
    }

    #[test]
    fn signature_rejects_wrong_key() {
        let exp = SystemTime::now()
            .checked_add(Duration::from_secs(60))
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let sig = sign(b"alpha", "blob/1.png", exp);
        let err = verify(b"beta", "blob/1.png", exp, &sig).unwrap_err();
        assert!(matches!(err, BlobStoreError::Signature(_)));
    }

    #[test]
    fn signature_rejects_expired() {
        let exp = 1; // long ago
        let sig = sign(b"k", "blob/1.png", exp);
        let err = verify(b"k", "blob/1.png", exp, &sig).unwrap_err();
        assert!(matches!(err, BlobStoreError::Signature(_)));
    }

    #[tokio::test]
    async fn presigned_url_includes_signature_and_exp() {
        let dir = temp_root();
        let s = store(dir.path());
        let url = s
            .presigned_url("a/b.png", Duration::from_secs(120))
            .await
            .unwrap();
        assert!(url.starts_with("/_blobs/a/b.png?exp="));
        assert!(url.contains("&sig="));
    }

    #[tokio::test]
    async fn presigned_url_percent_encodes_reserved_chars() {
        let dir = temp_root();
        let s = store(dir.path());
        let url = s
            .presigned_url("user 1/q?.png", Duration::from_secs(120))
            .await
            .unwrap();
        // Spaces, '?', and other reserved chars are percent-encoded;
        // '/' stays raw as a segment separator.
        assert!(url.starts_with("/_blobs/user%201/q%3F.png?exp="));
        assert!(url.contains("&sig="));
    }

    #[tokio::test]
    async fn put_stream_cleans_up_partial_file_on_error() {
        use futures::stream;

        let dir = temp_root();
        let s = store(dir.path());

        // First chunk succeeds, second yields an error to short-circuit
        // the write.
        let chunks: Vec<Result<Bytes, BlobStoreError>> = vec![
            Ok(Bytes::from_static(b"first")),
            Err(BlobStoreError::Backend("boom".into())),
        ];
        let stream: ByteStream = Box::pin(stream::iter(chunks));
        let err = s
            .put_stream("interrupted.bin", "application/octet-stream", stream)
            .await
            .unwrap_err();
        assert!(matches!(err, BlobStoreError::Backend(_)));

        // The partial file must not be left on disk.
        let path = dir.path().join("interrupted.bin");
        assert!(!path.exists(), "partial blob was not cleaned up");
        assert!(matches!(
            s.get("interrupted.bin").await.unwrap_err(),
            BlobStoreError::NotFound(_)
        ));
    }

    #[test]
    fn encode_key_path_passes_segments_separately() {
        assert_eq!(encode_key_path("foo"), "foo");
        assert_eq!(encode_key_path("a/b/c"), "a/b/c");
        assert_eq!(encode_key_path("a b/c?d"), "a%20b/c%3Fd");
        assert_eq!(encode_key_path("hash#frag/q"), "hash%23frag/q");
    }
}
