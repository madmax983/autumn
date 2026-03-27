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

/// Dynamic routes take priority over static files. When both exist,
/// the dynamic handler runs (SSR). Static files in dist/ are served
/// only for paths with NO matching dynamic route.
#[tokio::test]
async fn dynamic_routes_take_priority_over_static_files() {
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

    // Dynamic handler registered at the same path as the static file.
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

    // Dynamic handler wins — GET /about returns the SSR response, not the static file.
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
        "Dynamic handler should take priority, got: {text}"
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

/// POST requests must pass through ServeDir to the dynamic router,
/// even when a dist/ directory is active.
#[tokio::test]
async fn post_requests_pass_through_static_layer() {
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

    // Register both GET and POST on /admin
    let config = autumn_web::config::AutumnConfig::default();
    let router = autumn_web::app::build_router_with_static(
        vec![
            dynamic_get_route("/admin", "admin_list", "Admin Panel"),
            autumn_web::route::Route {
                method: http::Method::POST,
                path: "/admin",
                handler: axum::routing::post(|| async { "Created" }),
                name: "create",
            },
        ],
        &config,
        test_state(),
        Some(dist.as_path()),
    );

    // POST /admin should reach the dynamic handler, not get 405 from ServeDir
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "POST /admin should return 200, not 405 — ServeDir must not eat non-GET requests"
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(), "Created");
}
