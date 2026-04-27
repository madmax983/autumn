//! Integration tests for the avatars example.
//!
//! `survives_process_restart` is the headline test the storage spec
//! cares about: write a blob through one [`LocalBlobStore`], drop it,
//! point a fresh store at the same root, and confirm the bytes are
//! still there.

use std::sync::Arc;
use std::time::Duration;

use autumn_web::storage::{
    BlobStore, BlobStoreState, LocalBlobStore, SharedBlobStore, local::SigningKey,
};
use bytes::Bytes;

#[tokio::test]
async fn survives_process_restart() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::new(b"persistent-key".to_vec());

    // First "process": upload through the local store.
    {
        let store = LocalBlobStore::new(
            "default",
            dir.path().to_path_buf(),
            "/_blobs",
            Duration::from_secs(300),
            key.clone(),
        )
        .unwrap();
        let blob = store
            .put(
                "avatars/me.png",
                "image/png",
                Bytes::from_static(b"\x89PNG\r\n\x1a\nfake"),
            )
            .await
            .unwrap();
        assert_eq!(blob.byte_size, 12);
        assert_eq!(blob.provider_id, "default");
    }

    // Second "process": fresh store, same root, same key. Bytes still there.
    {
        let store = LocalBlobStore::new(
            "default",
            dir.path().to_path_buf(),
            "/_blobs",
            Duration::from_secs(300),
            key,
        )
        .unwrap();
        let bytes = store.get("avatars/me.png").await.unwrap();
        assert_eq!(&bytes[..], b"\x89PNG\r\n\x1a\nfake");
    }
}

#[tokio::test]
async fn presigned_url_round_trip_via_serving_route() {
    use autumn_web::reexports::axum::body::Body;
    use http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::new(b"serving-key".to_vec());
    let store = LocalBlobStore::new(
        "default",
        dir.path().to_path_buf(),
        "/_blobs",
        Duration::from_secs(60),
        key,
    )
    .unwrap();
    let blob = store
        .put("hello.txt", "text/plain", Bytes::from_static(b"hello"))
        .await
        .unwrap();

    let url = store
        .presigned_url(&blob.key, Duration::from_secs(120))
        .await
        .unwrap();

    // Mount the serving router and dispatch the signed URL through it.
    let arc: SharedBlobStore = Arc::new(store.clone());
    let state = autumn_web::AppState::for_test().with_extension(BlobStoreState::new(arc));
    let router = autumn_web::storage::local::serve_router(store).with_state(state);

    let request = Request::builder()
        .uri(&url)
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"hello");
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    use autumn_web::reexports::axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt as _;

    let dir = tempfile::tempdir().unwrap();
    let store = LocalBlobStore::new(
        "default",
        dir.path().to_path_buf(),
        "/_blobs",
        Duration::from_secs(60),
        SigningKey::new(b"the-key".to_vec()),
    )
    .unwrap();
    store
        .put("a.txt", "text/plain", Bytes::from_static(b"a"))
        .await
        .unwrap();

    let url = store
        .presigned_url("a.txt", Duration::from_secs(120))
        .await
        .unwrap();
    // Flip a hex digit in the signature.
    let tampered = if url.ends_with('0') {
        let len = url.len();
        format!("{}1", &url[..len - 1])
    } else {
        let len = url.len();
        format!("{}0", &url[..len - 1])
    };

    let arc: SharedBlobStore = Arc::new(store.clone());
    let state = autumn_web::AppState::for_test().with_extension(BlobStoreState::new(arc));
    let router = autumn_web::storage::local::serve_router(store).with_state(state);

    let request = Request::builder()
        .uri(&tampered)
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
