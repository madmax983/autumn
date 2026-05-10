//! Re-exports of Axum extractors for use in Autumn handlers.
//!
//! These are provided so users don't need `axum` as a direct dependency
//! for the most common extractor types.
//!
//! | Extractor | Purpose |
//! |-----------|---------|
//! | [`Form`] | Deserialize `application/x-www-form-urlencoded` request bodies |
//! | [`Json`] | Deserialize/serialize JSON request and response bodies |
//! | [`Path`] | Extract path parameters (e.g., `/users/{id}`) |
//! | [`Query`] | Deserialize URL query strings (e.g., `?page=2&limit=10`) |
//!
//! [`Json`] serves double duty -- it is both an extractor (parses JSON
//! request bodies) and a response type (serializes to JSON with
//! `Content-Type: application/json`).
//!
//! For the full set of Axum extractors, use
//! `autumn_web::reexports::axum::extract`.

/// Deserialize `application/x-www-form-urlencoded` request bodies.
///
/// Re-exported from [`axum::extract::Form`]. Commonly used with
/// HTML `<form>` submissions.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Login { username: String, password: String }
///
/// #[post("/login")]
/// async fn login(Form(input): Form<Login>) -> String {
///     format!("Welcome, {}!", input.username)
/// }
/// ```
pub use axum::extract::Form;

/// Deserialize and serialize JSON request/response bodies.
///
/// Re-exported from [`axum::extract::Json`]. As an extractor, parses the
/// request body. As a return type, serializes the value with
/// `Content-Type: application/json`.
///
/// Also available at the crate root as [`autumn_web::Json`](crate::Json).
pub use axum::extract::Json;

/// Extract typed path parameters from the URL.
///
/// Re-exported from [`axum::extract::Path`]. Use with route patterns
/// like `/users/{id}`.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::extract::Path;
///
/// #[get("/users/{id}")]
/// async fn get_user(Path(id): Path<i32>) -> String {
///     format!("User {id}")
/// }
/// ```
pub use axum::extract::Path;

/// Deserialize URL query string parameters.
///
/// Re-exported from [`axum::extract::Query`]. Parses the query string
/// into a typed struct.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::extract::Query;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Pagination { page: u32, limit: u32 }
///
/// #[get("/items")]
/// async fn list_items(Query(params): Query<Pagination>) -> String {
///     format!("Page {} (limit {})", params.page, params.limit)
/// }
/// ```
pub use axum::extract::Query;

/// Multipart form-data extractor with Autumn upload policy integration.
///
/// This wraps Axum's multipart extractor and applies framework-level
/// validation from `security.upload`:
///
/// - MIME allow-list checks (`allowed_mime_types`)
/// - Per-file size caps when consuming field bytes or streaming to disk
///
/// Request size limits are enforced per request in this extractor via
/// `security.upload.max_request_size_bytes`.
#[cfg(feature = "multipart")]
pub struct Multipart {
    inner: axum::extract::Multipart,
    config: crate::security::config::UploadConfig,
}

#[cfg(feature = "multipart")]
impl Multipart {
    /// Read the next multipart field, validating MIME type when configured.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AutumnError`] when multipart parsing fails or the
    /// field MIME type is not allowed by config.
    pub async fn next_field(&mut self) -> crate::AutumnResult<Option<MultipartField<'_>>> {
        let Some(field) = self
            .inner
            .next_field()
            .await
            .map_err(|err| multipart_error_to_error(&err))?
        else {
            return Ok(None);
        };

        // Only enforce MIME allow-lists for file parts. Regular form
        // fields often omit `Content-Type`.
        if field.file_name().is_some() && !self.config.allowed_mime_types.is_empty() {
            let Some(content_type) = field.content_type().map(str::to_owned) else {
                return Err(crate::AutumnError::bad_request_msg(
                    "missing content type on uploaded file",
                ));
            };
            if !self
                .config
                .allowed_mime_types
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&content_type))
            {
                return Err(crate::AutumnError::bad_request_msg(format!(
                    "unsupported upload content type: {content_type}"
                )));
            }
        }

        Ok(Some(MultipartField {
            inner: field,
            max_file_size_bytes: self.config.max_file_size_bytes,
        }))
    }
}

#[cfg(feature = "multipart")]
impl<S> axum::extract::FromRequest<S> for Multipart
where
    S: Send + Sync,
    axum::extract::Multipart:
        axum::extract::FromRequest<S, Rejection = axum::extract::multipart::MultipartRejection>,
{
    type Rejection = crate::AutumnError;

    async fn from_request(
        mut req: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let config = req
            .extensions()
            .get::<crate::security::config::UploadConfig>()
            .cloned()
            .unwrap_or_default();
        axum::extract::DefaultBodyLimit::max(config.max_request_size_bytes).apply(&mut req);
        let inner = axum::extract::Multipart::from_request(req, state)
            .await
            .map_err(|err| multipart_rejection_to_error(&err))?;
        Ok(Self { inner, config })
    }
}

/// A multipart field wrapper that provides safe streaming helpers.
#[cfg(feature = "multipart")]
pub struct MultipartField<'a> {
    inner: axum::extract::multipart::Field<'a>,
    max_file_size_bytes: usize,
}

#[cfg(all(feature = "multipart", feature = "storage"))]
struct MultipartFieldStreamState<'a> {
    inner: axum::extract::multipart::Field<'a>,
    total: usize,
    max: usize,
    errored: bool,
}

#[cfg(feature = "multipart")]
impl<'a> MultipartField<'a> {
    /// Field name from the multipart form.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.inner.name()
    }

    /// Uploaded file name (if this field represents a file).
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.inner.file_name()
    }

    /// Declared MIME type for this field.
    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        self.inner.content_type()
    }

    /// Tighten the per-field upload cap below the global
    /// `security.upload.max_file_size_bytes`.
    ///
    /// Routes can use this to enforce stricter caps than the global
    /// policy. The effective cap is `min(current, max)` — calling this
    /// with a value larger than the global is a no-op, so a route
    /// can't accidentally relax the framework-level limit.
    ///
    /// Returns `413 Payload Too Large` if subsequent reads exceed the
    /// tightened cap, just like the unchained form.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Cap avatar uploads at 2 MiB even if the global cap is higher.
    /// let blob = field
    ///     .with_max_bytes(2 * 1024 * 1024)
    ///     .save_to_blob_store(&*store, &key)
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_max_bytes(mut self, max: usize) -> Self {
        self.max_file_size_bytes = self.max_file_size_bytes.min(max);
        self
    }

    /// Read this field fully into memory while enforcing file-size limits.
    ///
    /// # Errors
    ///
    /// Returns `413 Payload Too Large` if the field exceeds
    /// `security.upload.max_file_size_bytes`.
    pub async fn bytes_limited(mut self) -> crate::AutumnResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut read = 0usize;
        while let Some(chunk) = self
            .inner
            .chunk()
            .await
            .map_err(|err| multipart_error_to_error(&err))?
        {
            read += chunk.len();
            if read > self.max_file_size_bytes {
                return Err(file_too_large_error(self.max_file_size_bytes));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Stream this field into a [`BlobStore`](crate::storage::BlobStore)
    /// while enforcing file-size limits.
    ///
    /// This is the production-ready replacement for [`save_to`](Self::save_to):
    /// the bytes flow through the configured blob backend (Local for dev,
    /// S3 for prod) so they survive container restarts and are visible to
    /// every replica.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use autumn_web::prelude::*;
    /// use autumn_web::extract::Multipart;
    /// use autumn_web::storage::BlobStoreState;
    ///
    /// #[post("/avatar")]
    /// async fn upload(state: State<AppState>, mut form: Multipart) -> AutumnResult<String> {
    ///     let store = state.extension::<BlobStoreState>().expect("storage configured");
    ///     while let Some(field) = form.next_field().await? {
    ///         if field.name() == Some("avatar") {
    ///             let blob = field
    ///                 .save_to_blob_store(store.store().as_ref(), "avatars/me.png")
    ///                 .await?;
    ///             return Ok(blob.key);
    ///         }
    ///     }
    ///     Err(autumn_web::AutumnError::bad_request_msg("missing avatar field"))
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when the field exceeds
    /// `security.upload.max_file_size_bytes`, when the multipart body is
    /// malformed, or when the underlying [`BlobStore`](crate::storage::BlobStore)
    /// rejects the write.
    #[cfg(feature = "storage")]
    pub async fn save_to_blob_store<'b>(
        self,
        store: &'b (dyn crate::storage::BlobStore + '_),
        key: impl Into<String>,
    ) -> crate::AutumnResult<crate::storage::Blob>
    where
        'a: 'b,
    {
        let key = key.into();
        let content_type = self
            .inner
            .content_type()
            .map_or_else(|| "application/octet-stream".to_owned(), str::to_owned);

        // Adapt the multipart chunk iterator into the trait's
        // `ByteStream`, enforcing the per-file size cap as we go so we
        // never buffer the whole upload in memory and large files flow
        // straight through to the store's streaming path.
        let state = MultipartFieldStreamState {
            inner: self.inner,
            total: 0,
            max: self.max_file_size_bytes,
            errored: false,
        };

        let stream = futures::stream::unfold(state, |mut state| async move {
            if state.errored {
                return None;
            }
            match state.inner.chunk().await {
                Ok(Some(chunk)) => {
                    state.total = state.total.saturating_add(chunk.len());
                    if state.total > state.max {
                        let err = crate::storage::BlobStoreError::PayloadTooLarge(format!(
                            "uploaded file exceeds limit of {} bytes",
                            state.max,
                        ));
                        state.errored = true;
                        Some((Err(err), state))
                    } else {
                        Some((Ok(chunk), state))
                    }
                }
                Ok(None) => None,
                Err(err) => {
                    // multer/axum already classifies multipart parser
                    // failures: 400 for malformed bodies, 413 for body
                    // limit violations, etc. Preserve that — wrapping
                    // every parser error as `Io` would silently turn
                    // client errors into 500s on the way out.
                    state.errored = true;
                    let mapped = blob_error_from_multipart(&err);
                    Some((Err(mapped), state))
                }
            }
        });
        let stream: crate::storage::ByteStream<'b> = Box::pin(stream);

        store
            .put_stream(&key, &content_type, stream)
            .await
            .map_err(crate::storage::BlobStoreError::into_autumn_error)
    }

    /// Stream this field to disk while enforcing file-size limits.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails or the file exceeds configured
    /// limits. Partial files are removed on limit violations.
    pub async fn save_to<P: AsRef<std::path::Path>>(
        mut self,
        path: P,
    ) -> crate::AutumnResult<usize> {
        use tokio::io::AsyncWriteExt as _;

        let path = path.as_ref();
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(crate::AutumnError::internal_server_error)?;

        let mut written = 0usize;
        while let Some(chunk) = self
            .inner
            .chunk()
            .await
            .map_err(|err| multipart_error_to_error(&err))?
        {
            written += chunk.len();
            if written > self.max_file_size_bytes {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(file_too_large_error(self.max_file_size_bytes));
            }
            file.write_all(&chunk)
                .await
                .map_err(crate::AutumnError::internal_server_error)?;
        }
        file.flush()
            .await
            .map_err(crate::AutumnError::internal_server_error)?;
        Ok(written)
    }
}

#[cfg(feature = "multipart")]
fn multipart_rejection_to_error(
    err: &axum::extract::multipart::MultipartRejection,
) -> crate::AutumnError {
    crate::AutumnError::bad_request_msg(err.body_text()).with_status(err.status())
}

#[cfg(feature = "multipart")]
/// Map a multipart parser error to a `BlobStoreError` variant whose
/// `status()` matches what the parser would have reported as an HTTP
/// response — so a malformed-body parser failure becomes 400, a body-
/// limit violation becomes 413, and only true server-side problems
/// stay as 500. Without this, every parser error would wrap as
/// `BlobStoreError::Io` and `into_autumn_error` would surface them all
/// as 500s.
#[cfg(all(feature = "multipart", feature = "storage"))]
fn blob_error_from_multipart(
    err: &axum::extract::multipart::MultipartError,
) -> crate::storage::BlobStoreError {
    let status = err.status();
    let body = err.body_text();
    if status == http::StatusCode::PAYLOAD_TOO_LARGE {
        crate::storage::BlobStoreError::PayloadTooLarge(body)
    } else if status.is_client_error() {
        crate::storage::BlobStoreError::InvalidInput(body)
    } else {
        crate::storage::BlobStoreError::Io(body)
    }
}

#[cfg(feature = "multipart")]
fn multipart_error_to_error(err: &axum::extract::multipart::MultipartError) -> crate::AutumnError {
    crate::AutumnError::bad_request_msg(err.body_text()).with_status(err.status())
}

#[cfg(feature = "multipart")]
fn file_too_large_error(max_file_size_bytes: usize) -> crate::AutumnError {
    crate::AutumnError::bad_request_msg(format!(
        "uploaded file exceeds limit of {max_file_size_bytes} bytes",
    ))
    .with_status(http::StatusCode::PAYLOAD_TOO_LARGE)
}

pub use axum::extract::State;

#[cfg(all(test, feature = "multipart"))]
mod tests {
    use super::*;
    use axum::extract::FromRequest;
    use axum::http::Request;

    #[tokio::test]
    async fn test_multipart_field_bytes_limited_success() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\r\nhello\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 100,
        };

        let bytes = wrapper.bytes_limited().await.unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn test_multipart_field_bytes_limited_too_large() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\r\nhello world\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 5,
        };

        let err = wrapper.bytes_limited().await.unwrap_err();
        assert_eq!(err.status(), http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_multipart_field_save_to_success() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\r\nfile content\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 100,
        };

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("out.txt");

        let written = wrapper.save_to(&file_path).await.unwrap();
        assert_eq!(written, 12);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "file content");
    }

    #[tokio::test]
    async fn test_multipart_field_save_to_too_large() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\r\nfile content\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 4,
        };

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("out_large.txt");

        let err = wrapper.save_to(&file_path).await.unwrap_err();
        assert_eq!(err.status(), http::StatusCode::PAYLOAD_TOO_LARGE);

        assert!(!file_path.exists());
    }

    #[cfg(feature = "storage")]
    #[tokio::test]
    async fn test_multipart_field_save_to_blob_store_success() {
        use crate::storage::{BlobStore, LocalBlobStore, local::SigningKey};
        use std::time::Duration;

        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nblob content\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 100,
        };

        let root = tempfile::tempdir().unwrap();
        let store = LocalBlobStore::new(
            "local",
            root.path(),
            "/blobs",
            Duration::from_secs(3600),
            SigningKey::random(),
            vec![],
        )
        .unwrap();

        let blob = wrapper.save_to_blob_store(&store, "myblob").await.unwrap();
        assert_eq!(blob.key, "myblob");
        assert_eq!(blob.content_type, "text/plain");

        let bytes = store.get("myblob").await.unwrap();
        assert_eq!(&bytes[..], b"blob content");
    }

    #[cfg(feature = "storage")]
    #[tokio::test]
    async fn test_multipart_field_save_to_blob_store_too_large() {
        use crate::storage::{BlobStore, LocalBlobStore, local::SigningKey};
        use std::time::Duration;

        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nblob content\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 4, // "blob content" is 12 bytes
        };

        let root = tempfile::tempdir().unwrap();
        let store = LocalBlobStore::new(
            "local",
            root.path(),
            "/blobs",
            Duration::from_secs(3600),
            SigningKey::random(),
            vec![],
        )
        .unwrap();

        let err = wrapper
            .save_to_blob_store(&store, "myblob")
            .await
            .unwrap_err();
        assert_eq!(err.status(), http::StatusCode::PAYLOAD_TOO_LARGE);

        // Verify that the blob was not created/persisted
        let get_err = store.get("myblob").await.unwrap_err();
        assert_eq!(get_err.status(), http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_multipart_field_metadata() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"custom_name\"; filename=\"custom_file.png\"\r\nContent-Type: image/png\r\n\r\npng\r\n--boundary--\r\n";
        let req = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();

        let mut multipart = axum::extract::Multipart::from_request(req, &())
            .await
            .unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();

        let wrapper = MultipartField {
            inner: field,
            max_file_size_bytes: 100,
        };

        assert_eq!(wrapper.name(), Some("custom_name"));
        assert_eq!(wrapper.file_name(), Some("custom_file.png"));
        assert_eq!(wrapper.content_type(), Some("image/png"));

        let tighter = wrapper.with_max_bytes(50);
        assert_eq!(tighter.max_file_size_bytes, 50);

        let not_tighter = tighter.with_max_bytes(200);
        assert_eq!(not_tighter.max_file_size_bytes, 50); // should not relax
    }
}
