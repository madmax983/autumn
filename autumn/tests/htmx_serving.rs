#![cfg(feature = "htmx")]
//! Tests for htmx embedding and serving.

#[test]
fn htmx_version_is_accessible() {
    let version = autumn_web::HTMX_VERSION;
    assert!(!version.is_empty());
    assert!(
        version.starts_with("2."),
        "Expected htmx 2.x, got {version}"
    );
}
