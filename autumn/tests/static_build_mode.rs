//! Integration test: verify `render_static_routes` works with a real
//! Autumn router (not just a mock fallback handler).

use autumn_web::app::build_router;
use autumn_web::config::AutumnConfig;
use autumn_web::route::Route;
use autumn_web::static_gen::{StaticRouteMeta, render_static_routes};

fn about_route() -> Route {
    Route {
        method: http::Method::GET,
        path: "/about",
        handler: axum::routing::get(|| async { "About Page Content" }),
        name: "about",
    }
}

const fn about_meta() -> StaticRouteMeta {
    StaticRouteMeta {
        path: "/about",
        name: "about",
        revalidate: None,
    }
}

fn test_state() -> autumn_web::AppState {
    autumn_web::AppState {
        #[cfg(feature = "db")]
        pool: None,
        profile: None,
        started_at: std::time::Instant::now(),
        health_detailed: false,
    }
}

#[tokio::test]
async fn build_mode_renders_through_real_router() {
    let config = AutumnConfig::default();
    let router = build_router(vec![about_route()], &config, test_state());

    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("dist");

    let result = render_static_routes(router, &[about_meta()], &dist).await;
    assert!(result.is_ok(), "build failed: {:?}", result.err());

    let html = std::fs::read_to_string(dist.join("about/index.html")).unwrap();
    assert_eq!(html, "About Page Content");

    // Verify manifest
    let manifest =
        autumn_web::static_gen::StaticManifest::load(&dist.join("manifest.json")).unwrap();
    assert_eq!(manifest.routes.len(), 1);
    assert!(manifest.routes.contains_key("/about"));
}
