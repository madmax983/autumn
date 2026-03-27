//! Integration tests for static-file-first serving through the full router.
//!
//! Exercises `build_router_with_static` end-to-end: static files shadow
//! dynamic routes when a `dist/` directory with a valid manifest is present,
//! and dynamic routes still work when no static build exists.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::collections::HashMap;
use tower::ServiceExt;

/// Helper: build an `AppState` suitable for testing (no database, no profile).
fn test_state() -> autumn_web::AppState {
    autumn_web::AppState {
        #[cfg(feature = "db")]
        pool: None,
        profile: None,
        started_at: std::time::Instant::now(),
        health_detailed: true,
    }
}

/// Helper: create a `Route` for a GET handler that returns the given body.
fn dynamic_get_route(
    path: &'static str,
    name: &'static str,
    body: &'static str,
) -> autumn_web::route::Route {
    autumn_web::route::Route {
        method: http::Method::GET,
        path,
        handler: axum::routing::get(move || async move { body }),
        name,
    }
}

/// When a `dist/` directory contains a static file for a route, the static
/// file takes priority over a dynamic handler registered at the same path.
#[tokio::test]
async fn static_files_served_over_dynamic_routes() {
    // 1. Set up a temp dist/ with about/index.html and a valid manifest.json
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    std::fs::create_dir_all(dist.join("about")).expect("mkdir about");
    std::fs::write(dist.join("about/index.html"), "<h1>Static About</h1>").expect("write html");

    let manifest = autumn_web::static_gen::StaticManifest {
        generated_at: "2026-03-27T00:00:00Z".to_owned(),
        autumn_version: "0.1.0".to_owned(),
        routes: HashMap::from([(
            "/about".to_owned(),
            autumn_web::static_gen::ManifestEntry {
                file: "about/index.html".to_owned(),
                revalidate: None,
            },
        )]),
    };
    std::fs::write(
        dist.join("manifest.json"),
        serde_json::to_string(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");

    // 2. Build a router where the dynamic handler would return different text.
    let config = autumn_web::config::AutumnConfig::default();
    let router = autumn_web::app::build_router_with_static(
        vec![dynamic_get_route(
            "/about",
            "about_dynamic",
            "Dynamic About",
        )],
        &config,
        test_state(),
        Some(dist.as_path()),
    );

    // 3. GET /about redirects to /about/ (307), then serves index.html.
    //    ServeDir issues a 307 redirect for directory paths without
    //    trailing slash, then serves the index.html on the redirected
    //    request. Browsers follow this automatically.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/about")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "ServeDir should redirect /about to /about/"
    );

    // 4. Following the redirect: GET /about/ returns the static content.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/about/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).expect("valid utf-8");
    assert!(
        text.contains("Static About"),
        "Expected static file content, got: {text}"
    );
}

/// When no dist directory is provided, dynamic routes work normally.
#[tokio::test]
async fn dynamic_routes_still_work_without_dist() {
    let config = autumn_web::config::AutumnConfig::default();
    let router = autumn_web::app::build_router_with_static(
        vec![dynamic_get_route(
            "/about",
            "about_dynamic",
            "Dynamic About",
        )],
        &config,
        test_state(),
        None,
    );

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/about")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).expect("valid utf-8");
    assert!(
        text.contains("Dynamic About"),
        "Expected dynamic handler response, got: {text}"
    );
}

/// Routes not present in the static manifest fall through to the dynamic
/// router, even when a dist directory is active.
#[tokio::test]
async fn unknown_routes_fall_through_to_dynamic() {
    // 1. Create a dist/ that only knows about /about.
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    std::fs::create_dir_all(dist.join("about")).expect("mkdir about");
    std::fs::write(dist.join("about/index.html"), "<h1>Static About</h1>").expect("write html");

    let manifest = autumn_web::static_gen::StaticManifest {
        generated_at: "2026-03-27T00:00:00Z".to_owned(),
        autumn_version: "0.1.0".to_owned(),
        routes: HashMap::from([(
            "/about".to_owned(),
            autumn_web::static_gen::ManifestEntry {
                file: "about/index.html".to_owned(),
                revalidate: None,
            },
        )]),
    };
    std::fs::write(
        dist.join("manifest.json"),
        serde_json::to_string(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");

    // 2. Build router with a dynamic /admin route (not in the manifest).
    let config = autumn_web::config::AutumnConfig::default();
    let router = autumn_web::app::build_router_with_static(
        vec![dynamic_get_route("/admin", "admin", "Admin Panel")],
        &config,
        test_state(),
        Some(dist.as_path()),
    );

    // 3. GET /admin should fall through to the dynamic handler.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).expect("valid utf-8");
    assert!(
        text.contains("Admin Panel"),
        "Expected dynamic handler response, got: {text}"
    );
}
