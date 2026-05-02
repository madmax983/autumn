#![cfg(feature = "storage")]

use std::env;
use std::process::Command;

use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str {
    "ok"
}

#[test]
fn s3_configuration_fails_before_http_listener_is_ready() {
    let current_exe = env::current_exe().expect("current test executable path");
    let output = Command::new(current_exe)
        .arg("--exact")
        .arg("child_boots_with_s3_storage")
        .arg("--nocapture")
        .env("AUTUMN_S3_PREFLIGHT_CHILD", "1")
        .env("AUTUMN_PROFILE", "prod")
        .env("AUTUMN_STORAGE__BACKEND", "s3")
        .env("AUTUMN_STORAGE__S3__BUCKET", "uploads")
        .env("AUTUMN_STORAGE__S3__REGION", "us-east-1")
        .env("AUTUMN_LOG__LEVEL", "info")
        .env("AUTUMN_SERVER__PORT", "0")
        .output()
        .expect("spawn child test process");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let logs = format!("{stdout}\n{stderr}");

    assert!(
        !output.status.success(),
        "unsupported S3 config must abort startup; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        logs.contains("storage.backend=s3"),
        "diagnostic should name selected backend; output:\n{logs}"
    );
    assert!(
        logs.contains("issue #530"),
        "diagnostic should point to the real S3 provider path; output:\n{logs}"
    );
    assert!(
        logs.contains("storage.backend=local"),
        "diagnostic should name the local fallback for acknowledged single-replica deployments; output:\n{logs}"
    );
    assert!(
        !logs.contains("Listening"),
        "storage preflight must fail before the listener is ready; output:\n{logs}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn child_boots_with_s3_storage() {
    if env::var_os("AUTUMN_S3_PREFLIGHT_CHILD").is_none() {
        return;
    }

    autumn_web::app().routes(routes![index]).run().await;
}
