use std::fs;
use std::path::Path;

const ACTIVE_DOCS: &[&str] = &[
    "README.md",
    "docs/guide/storage.md",
    "docs/guide/getting-started.md",
    "docs/guide/cloud-native.md",
];

#[test]
fn active_storage_docs_do_not_advertise_s3_as_available() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");

    for doc in ACTIVE_DOCS {
        let body = fs::read_to_string(workspace.join(doc)).expect("read active storage doc");
        assert!(
            !body.contains("built-in `Local` and S3-compatible backends"),
            "{doc} still describes S3 as a built-in available backend"
        );
        assert!(
            !body.contains("Multi-replica production should choose `backend = \"s3\"`"),
            "{doc} still recommends S3 for production before #530 lands"
        );
        assert!(
            !body.contains("pick the `S3` backend"),
            "{doc} still tells users to pick S3 before #530 lands"
        );
    }
}

#[test]
fn storage_guide_names_local_support_and_planned_s3_gate() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let storage =
        fs::read_to_string(workspace.join("docs/guide/storage.md")).expect("read storage guide");

    assert!(
        storage.contains("Local is the only supported built-in storage backend today"),
        "storage guide should state the currently supported backend"
    );
    assert!(
        storage.contains("planned S3 plugin path tracked by issue #530"),
        "storage guide should point S3 users to #530"
    );
    assert!(
        storage.contains("upload/download/delete/presigned-URL"),
        "storage guide should keep the future S3 smoke-gate visible"
    );
}
