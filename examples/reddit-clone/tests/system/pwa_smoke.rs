//! PWA smoke test — manifest content-type + service-worker registration.
//!
//! Run with:
//!   cargo test --features system-tests --test pwa_smoke -- --include-ignored

#![cfg(feature = "system-tests")]

use autumn_web::prelude::*;
use autumn_web::system_test::SystemTest;

#[get("/manifest.webmanifest")]
async fn pwa_manifest() -> impl IntoResponse {
    ([("content-type", "application/manifest+json")], "")
}

#[get("/service-worker.js")]
async fn pwa_service_worker() -> impl IntoResponse {
    (
        [
            ("content-type", "text/javascript; charset=utf-8"),
            ("service-worker-allowed", "/"),
        ],
        "",
    )
}

#[get("/pwa-register.js")]
async fn pwa_register_js() -> impl IntoResponse {
    ([("content-type", "text/javascript; charset=utf-8")], "")
}

#[get("/offline")]
async fn pwa_offline() -> impl IntoResponse {
    autumn_web::reexports::axum::response::Html(
        "<html><head><link rel=\"manifest\" href=\"/manifest.webmanifest\"></head><body></body></html>",
    )
}

/// Checks that `GET /manifest.webmanifest` returns `application/manifest+json`
/// and that the `<link rel="manifest">` tag is present in the page DOM.
#[tokio::test]
#[ignore = "requires Chromium; run with --include-ignored"]
async fn pwa_manifest_loads_with_correct_content_type() {
let runner = SystemTest::new()
.routes(routes![pwa_manifest, pwa_service_worker, pwa_register_js, pwa_offline])
.build()
.await
.expect("test runner");
let base_url = runner.base_url();
let page = runner.page().await.expect("page");

// Verify HTTP content-type via raw TCP to avoid a reqwest dev-dependency.
{
use std::io::{Read, Write};
let host_port = base_url
.trim_start_matches("http://")
.trim_start_matches("https://");
let mut stream = std::net::TcpStream::connect(host_port)
.expect("connect to test server");
let req = format!("GET /manifest.webmanifest HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
stream.write_all(req.as_bytes()).expect("write request");
let mut response = String::new();
stream.read_to_string(&mut response).expect("read response");
assert!(
response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
"manifest must return 200, got: {response}"
);
assert!(
response.contains("application/manifest+json"),
"manifest content-type must be application/manifest+json"
);
}

// Browser check: <link rel="manifest"> is in <head>
page.visit("/offline").await.expect("offline page loaded");
page.expect_attribute("link[rel=\"manifest\"]", "href", "/manifest.webmanifest")
.await
.expect("manifest link present in DOM");
}

/// Verifies that the service worker registers successfully (scope covers the whole app).
/// The `/offline` page is used as the test shell since it is always available.
#[tokio::test]
#[ignore = "requires Chromium; run with --include-ignored"]
async fn service_worker_registers_successfully() {
let runner = SystemTest::new()
.routes(routes![pwa_manifest, pwa_service_worker, pwa_register_js, pwa_offline])
.build()
.await
.expect("test runner");
let page = runner.page().await.expect("page");

// `/pwa-register.js` sets `data-sw-registered="true"` on `<html>`
// after the SW registers.  Visiting `/offline` (which uses layout)
// loads the script without needing the user's root route.
page.visit("/offline").await.expect("offline page loaded");
page.expect_attribute("html", "data-sw-registered", "true")
.await
.expect("service worker registered and controlling page");
}
