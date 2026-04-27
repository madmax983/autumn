#![cfg(feature = "mail")]

use autumn_web::config::{AutumnConfig, MockEnv};
use autumn_web::mail::{Mail, Mailer, Transport};
use autumn_web::prelude::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt as _;

#[test]
fn dev_profile_defaults_to_log_transport() {
    let env = MockEnv::new().with("AUTUMN_PROFILE", "dev");

    let config = AutumnConfig::load_with_env(&env).expect("dev config should load");

    assert_eq!(config.mail.transport, Transport::Log);
}

#[test]
fn prod_profile_rejects_log_transport_without_explicit_acknowledgement() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("autumn.toml"),
        r#"
[mail]
transport = "log"
from = "noreply@example.com"
"#,
    )
    .expect("write config");
    let env = MockEnv::new()
        .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap())
        .with("AUTUMN_PROFILE", "prod");

    let error = AutumnConfig::load_with_env(&env).expect_err("prod log mail must fail");

    assert!(
        error.to_string().contains("mail.allow_log_in_production"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn file_transport_writes_rfc822_message_for_inspection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mailer = Mailer::builder()
        .from("Autumn <noreply@example.com>")
        .transport(Transport::File)
        .file_dir(dir.path())
        .build()
        .expect("file mailer should build");
    let mail = Mail::builder()
        .to("Ada Lovelace <ada@example.com>")
        .subject("Reset your password")
        .html("<p>Use code 123456</p>")
        .text("Use code 123456")
        .build()
        .expect("mail should build");

    mailer.send(mail).await.expect("file send should succeed");

    let files = std::fs::read_dir(dir.path())
        .expect("mail dir exists")
        .collect::<Result<Vec<_>, _>>()
        .expect("mail dir readable");
    assert_eq!(files.len(), 1);
    let body = std::fs::read_to_string(files[0].path()).expect("eml readable");
    assert!(body.contains("To: Ada Lovelace <ada@example.com>"));
    assert!(body.contains("From: Autumn <noreply@example.com>"));
    assert!(body.contains("Subject: Reset your password"));
    assert!(body.contains("Use code 123456"));
}

#[tokio::test]
async fn mailer_is_a_cloneable_handler_extractor() {
    async fn send(mailer: Mailer) -> AutumnResult<&'static str> {
        let mail = Mail::builder()
            .to("user@example.com")
            .subject("Hello")
            .text("hello")
            .build()?;
        mailer.send(mail).await?;
        Ok("sent")
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let mailer = Mailer::builder()
        .from("noreply@example.com")
        .transport(Transport::File)
        .file_dir(dir.path())
        .build()
        .expect("mailer should build");
    let state = AppState::for_test().with_extension(mailer);
    let router = axum::Router::new()
        .route("/send", axum::routing::get(send))
        .with_state(state);

    let response = router
        .oneshot(Request::builder().uri("/send").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("mail dir exists")
            .count(),
        1
    );
}
