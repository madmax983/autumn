//!
//! Integration tests for extractors.
//!
use autumn_web::extract::{Form, Json, Multipart};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

// ── Json tests ───────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonInput {
    value: i32,
}

#[derive(Serialize)]
struct JsonOutput {
    doubled: i32,
}

#[tokio::test]
async fn json_extractor_and_response() {
    async fn handler(Json(input): Json<JsonInput>) -> Json<JsonOutput> {
        Json(JsonOutput {
            doubled: input.value * 2,
        })
    }

    let app = Router::new().route("/", axum::routing::post(handler));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"value": 21}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("application/json"));

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let output: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(output["doubled"], 42);
}

#[tokio::test]
async fn invalid_json_returns_error() {
    async fn handler(Json(_): Json<serde_json::Value>) -> &'static str {
        "ok"
    }

    let app = Router::new().route("/", axum::routing::post(handler));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from("not json"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ── Form tests ───────────────────────────────────────────────

#[derive(Deserialize)]
struct FormInput {
    name: String,
    age: u32,
}

#[tokio::test]
async fn form_extraction_works() {
    async fn handler(Form(data): Form<FormInput>) -> String {
        format!("{} is {}", data.name, data.age)
    }

    let app = Router::new().route("/", axum::routing::post(handler));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Alice&age=30"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"Alice is 30");
}

#[tokio::test]
async fn multipart_extraction_works() {
    async fn handler(mut multipart: Multipart) -> autumn_web::AutumnResult<String> {
        let field = multipart
            .next_field()
            .await?
            .expect("expected one multipart field");
        let name = field.file_name().unwrap_or("missing").to_string();
        let body = field.bytes_limited().await?;
        Ok(format!("{name}:{}", body.len()))
    }

    let boundary = "X-BOUNDARY";
    let payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n"
    );

    let app = Router::new().route("/", axum::routing::post(handler));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"hello.txt:11");
}

#[tokio::test]
async fn multipart_mime_allow_list_skips_non_file_fields() {
    async fn handler(mut multipart: Multipart) -> autumn_web::AutumnResult<String> {
        let mut text_seen = false;
        let mut file_seen = false;

        while let Some(field) = multipart.next_field().await? {
            if field.file_name().is_some() {
                file_seen = true;
            } else {
                text_seen = true;
            }
            let _ = field.bytes_limited().await?;
        }

        Ok(format!("text={text_seen},file={file_seen}"))
    }

    let boundary = "X-BOUNDARY";
    let payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nreport\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n"
    );

    let app = Router::new()
        .route("/", axum::routing::post(handler))
        .layer(axum::middleware::from_fn(
            |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                req.extensions_mut()
                    .insert(autumn_web::security::config::UploadConfig {
                        allowed_mime_types: vec!["text/plain".to_owned()],
                        ..autumn_web::security::config::UploadConfig::default()
                    });
                next.run(req).await
            },
        ));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"text=true,file=true");
}

#[tokio::test]
async fn multipart_file_size_limit_returns_413() {
    async fn handler(mut multipart: Multipart) -> autumn_web::AutumnResult<&'static str> {
        let field = multipart.next_field().await?.expect("expected field");
        let _ = field.bytes_limited().await?;
        Ok("ok")
    }

    let boundary = "X-BOUNDARY";
    let payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n"
    );

    let app = Router::new()
        .route("/", axum::routing::post(handler))
        .layer(axum::middleware::from_fn(
            |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                req.extensions_mut()
                    .insert(autumn_web::security::config::UploadConfig {
                        max_file_size_bytes: 4,
                        ..autumn_web::security::config::UploadConfig::default()
                    });
                next.run(req).await
            },
        ));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn multipart_request_size_limit_returns_413() {
    async fn handler(mut multipart: Multipart) -> autumn_web::AutumnResult<&'static str> {
        let _ = multipart.next_field().await?;
        Ok("ok")
    }

    let boundary = "X-BOUNDARY";
    let payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n"
    );

    let app = Router::new()
        .route("/", axum::routing::post(handler))
        .layer(axum::middleware::from_fn(
            |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                req.extensions_mut()
                    .insert(autumn_web::security::config::UploadConfig {
                        max_request_size_bytes: 8,
                        ..autumn_web::security::config::UploadConfig::default()
                    });
                next.run(req).await
            },
        ));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
