//! Integration tests for `OpenAPI` auto-generation (S-056).
//!
//! Covers:
//! * Route macros emit the expected `ApiDoc` metadata.
//! * `#[api_doc(...)]` overrides flow through to the generated spec.
//! * `AppBuilder::openapi(...)` mounts `/v3/api-docs` and `/swagger-ui`.

use autumn_web::Route;
use autumn_web::openapi::{ApiDoc, OpenApiConfig, SchemaKind};
use autumn_web::prelude::*;
use autumn_web::test::TestApp;

// ── Route-level metadata extraction ────────────────────────────────

#[get("/hello")]
async fn hello() -> &'static str {
    "hi"
}

#[get("/users/{id}")]
async fn get_user(Path(id): Path<i32>) -> String {
    format!("User {id}")
}

#[get("/posts/{year}/{slug}")]
async fn get_post(_params: Path<(i32, String)>) -> &'static str {
    "post"
}

#[post("/items")]
async fn create_item(Json(body): Json<serde_json::Value>) -> axum::Json<serde_json::Value> {
    axum::Json(body)
}

#[get("/admin")]
#[api_doc(summary = "Admin area", tag = "admin", status = 201)]
async fn admin() -> &'static str {
    "admin"
}

#[get("/hidden")]
#[api_doc(hidden)]
async fn hidden_route() -> &'static str {
    "hidden"
}

#[get("/tagged")]
#[api_doc(tags = ["users", "auth"], description = "Multi-tagged route")]
async fn tagged() -> &'static str {
    "tagged"
}

#[test]
fn get_macro_populates_api_doc() {
    let route = __autumn_route_info_hello();
    assert_eq!(route.api_doc.method, "GET");
    assert_eq!(route.api_doc.path, "/hello");
    assert_eq!(route.api_doc.operation_id, "hello");
    assert_eq!(route.api_doc.success_status, 200);
    assert!(!route.api_doc.hidden);
    assert!(route.api_doc.path_params.is_empty());
}

#[test]
fn path_parameters_are_extracted() {
    let route = __autumn_route_info_get_user();
    assert_eq!(route.api_doc.path_params, &["id"]);
}

#[test]
fn multiple_path_parameters_are_extracted() {
    let route = __autumn_route_info_get_post();
    assert_eq!(route.api_doc.path_params, &["year", "slug"]);
}

#[test]
fn json_request_body_is_inferred() {
    let route = __autumn_route_info_create_item();
    let body = route
        .api_doc
        .request_body
        .as_ref()
        .expect("Json<...> body should be inferred");
    assert_eq!(body.name, "Value");
    assert_eq!(body.kind, SchemaKind::Ref);
}

#[test]
fn json_response_is_inferred() {
    let route = __autumn_route_info_create_item();
    let resp = route
        .api_doc
        .response
        .as_ref()
        .expect("Json<...> return should be inferred");
    assert_eq!(resp.name, "Value");
}

#[test]
fn api_doc_attribute_applies_summary_and_tag() {
    let route = __autumn_route_info_admin();
    assert_eq!(route.api_doc.summary, Some("Admin area"));
    assert_eq!(route.api_doc.tags, &["admin"]);
    assert_eq!(route.api_doc.success_status, 201);
}

#[test]
fn api_doc_attribute_can_hide_route() {
    let route = __autumn_route_info_hidden_route();
    assert!(route.api_doc.hidden);
}

#[test]
fn api_doc_attribute_accepts_tag_list() {
    let route = __autumn_route_info_tagged();
    assert_eq!(route.api_doc.tags, &["users", "auth"]);
    assert_eq!(route.api_doc.description, Some("Multi-tagged route"));
}

// ── Spec generation pipeline ───────────────────────────────────────

#[test]
fn generate_spec_emits_paths_for_every_method() {
    let routes: Vec<Route> = routes![hello, get_user, create_item, admin, hidden_route];
    let docs: Vec<&ApiDoc> = routes.iter().map(|r| &r.api_doc).collect();
    let config = OpenApiConfig::new("Test API", "0.1.0");
    let spec = autumn_web::openapi::generate_spec(&config, &docs);

    assert_eq!(spec.info.title, "Test API");
    assert_eq!(spec.info.version, "0.1.0");
    assert!(spec.paths.contains_key("/hello"));
    assert!(spec.paths.contains_key("/users/{id}"));
    assert!(spec.paths.contains_key("/items"));
    assert!(spec.paths.contains_key("/admin"));
    assert!(
        !spec.paths.contains_key("/hidden"),
        "`#[api_doc(hidden)]` should exclude routes"
    );

    // Admin returned a 201
    let admin_op = spec.paths["/admin"].get.as_ref().unwrap();
    assert!(admin_op.responses.contains_key("201"));
    assert_eq!(admin_op.summary.as_deref(), Some("Admin area"));

    // Path parameter surfaced
    let user_op = spec.paths["/users/{id}"].get.as_ref().unwrap();
    assert_eq!(user_op.parameters.len(), 1);
    assert_eq!(user_op.parameters[0].name, "id");
}

// ── Endpoint integration ──────────────────────────────────────────

#[tokio::test]
async fn openapi_json_endpoint_returns_spec() {
    let client = TestApp::new()
        .routes(routes![hello, get_user])
        .openapi(OpenApiConfig::new("Demo", "1.0.0"))
        .build();

    let response = client.get("/v3/api-docs").send().await;
    response.assert_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["openapi"], "3.0.3");
    assert_eq!(body["info"]["title"], "Demo");
    assert!(body["paths"]["/hello"].is_object());
    assert!(body["paths"]["/users/{id}"].is_object());
}

#[tokio::test]
async fn swagger_ui_endpoint_returns_html_referencing_spec_url() {
    let client = TestApp::new()
        .routes(routes![hello])
        .openapi(OpenApiConfig::new("Demo", "1.0.0"))
        .build();

    let response = client.get("/swagger-ui").send().await;
    response.assert_ok();

    let body = response.text();
    assert!(body.contains("SwaggerUIBundle"));
    assert!(body.contains("/v3/api-docs"));
}

#[tokio::test]
async fn openapi_not_mounted_without_explicit_call() {
    let client = TestApp::new().routes(routes![hello]).build();
    let response = client.get("/v3/api-docs").send().await;
    assert_eq!(
        response.status,
        http::StatusCode::NOT_FOUND,
        "/v3/api-docs should 404 until AppBuilder::openapi(...) is called"
    );
}

#[tokio::test]
async fn custom_openapi_paths_are_honored() {
    let config = OpenApiConfig::new("Demo", "1.0.0")
        .openapi_json_path("/openapi.json")
        .swagger_ui_path(Some("/docs".to_owned()));

    let client = TestApp::new()
        .routes(routes![hello])
        .openapi(config)
        .build();

    client.get("/openapi.json").send().await.assert_ok();
    client.get("/docs").send().await.assert_ok();

    // The default paths should now return 404
    let default_json = client.get("/v3/api-docs").send().await;
    assert_eq!(default_json.status, http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn swagger_ui_can_be_disabled() {
    let config = OpenApiConfig::new("Demo", "1.0.0").swagger_ui_path(None);
    let client = TestApp::new()
        .routes(routes![hello])
        .openapi(config)
        .build();

    client.get("/v3/api-docs").send().await.assert_ok();
    let ui = client.get("/swagger-ui").send().await;
    assert_eq!(ui.status, http::StatusCode::NOT_FOUND);
}
