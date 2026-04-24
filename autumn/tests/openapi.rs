//! Integration tests for `OpenAPI` auto-generation (S-056).
//!
//! Covers:
//! * Route macros emit the expected `ApiDoc` metadata.
//! * `#[api_doc(...)]` overrides flow through to the generated spec.
//! * `AppBuilder::openapi(...)` mounts `/v3/api-docs` and `/swagger-ui`.

#![cfg(feature = "openapi")]

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

// Exercise the *reversed* attribute order: `#[api_doc]` above `#[get]`.
// Rust expands `#[api_doc]` first; the standalone macro must reorder
// so the route macro still sees the overrides.
#[api_doc(summary = "Top-first api_doc", tag = "top")]
#[get("/top-first")]
async fn top_first() -> &'static str {
    "top"
}

#[api_doc(hidden)]
#[post("/top-hidden")]
async fn top_hidden() -> &'static str {
    "hidden"
}

// Responses wrapped in `(StatusCode, Json<T>)` — common for 201 Created
// handlers — should still be inferred.
#[post("/things")]
async fn create_thing() -> (http::StatusCode, axum::Json<serde_json::Value>) {
    (http::StatusCode::CREATED, axum::Json(serde_json::json!({})))
}

// Qualified form: `#[api_doc(...)]` on top, `#[autumn_web::get(...)]`
// qualified below. The reorder helper must recognize the qualified
// path via its last segment so the overrides still flow through.
#[api_doc(summary = "Fully-qualified get")]
#[autumn_web::get("/qualified")]
async fn qualified_get() -> &'static str {
    "ok"
}

// `Json<Vec<T>>` — the generator must emit an array schema rather
// than collapsing to a `$ref` to `Vec`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Widget {
    id: i32,
}

#[get("/widgets")]
async fn list_widgets() -> axum::Json<Vec<Widget>> {
    axum::Json(vec![])
}

#[post("/widgets")]
async fn post_widgets(axum::Json(_body): axum::Json<Vec<Widget>>) -> http::StatusCode {
    http::StatusCode::OK
}

// `Valid<Json<T>>` is Autumn's documented validation pattern. The
// generator must see straight through the wrapper so the resulting
// spec still reports a request body.
#[derive(serde::Deserialize, serde::Serialize, validator::Validate)]
struct NewWidget {
    #[validate(length(min = 1))]
    name: String,
}

#[post("/validated-widgets")]
async fn create_validated_widget(
    _body: autumn_web::Valid<autumn_web::Json<NewWidget>>,
) -> http::StatusCode {
    http::StatusCode::CREATED
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

#[test]
fn api_doc_survives_when_placed_above_route_attribute() {
    let route = __autumn_route_info_top_first();
    assert_eq!(
        route.api_doc.summary,
        Some("Top-first api_doc"),
        "`#[api_doc]` above `#[get]` must not be dropped"
    );
    assert_eq!(route.api_doc.tags, &["top"]);
}

#[test]
fn api_doc_hidden_survives_when_placed_above_route_attribute() {
    let route = __autumn_route_info_top_hidden();
    assert!(route.api_doc.hidden);
}

#[test]
fn status_tuple_response_is_inferred_as_json() {
    let route = __autumn_route_info_create_thing();
    let resp = route
        .api_doc
        .response
        .as_ref()
        .expect("(StatusCode, Json<T>) should be inferred");
    assert_eq!(resp.name, "Value");
    assert_eq!(resp.kind, SchemaKind::Ref);
}

#[test]
fn api_doc_survives_above_qualified_route_attribute() {
    let route = __autumn_route_info_qualified_get();
    assert_eq!(
        route.api_doc.summary,
        Some("Fully-qualified get"),
        "qualified route attr should still be detected by the reorder helper"
    );
}

#[test]
fn json_vec_response_is_emitted_as_array_schema() {
    let route = __autumn_route_info_list_widgets();
    let resp = route
        .api_doc
        .response
        .as_ref()
        .expect("Json<Vec<T>> must infer a response");
    assert!(
        matches!(resp.kind, SchemaKind::Array(_)),
        "Json<Vec<T>> must become Array, got {:?}",
        resp.kind
    );

    // Render through the spec generator to confirm the actual JSON.
    let config = OpenApiConfig::new("Demo", "1.0.0");
    let spec = autumn_web::openapi::generate_spec(&config, &[&route.api_doc]);
    let media =
        &spec.paths["/widgets"].get.as_ref().unwrap().responses["200"].content["application/json"];
    assert_eq!(media.schema["type"], "array");
    assert_eq!(
        media.schema["items"]["$ref"], "#/components/schemas/Widget",
        "array items must still ref the element type"
    );
}

#[test]
fn json_vec_request_body_is_emitted_as_array_schema() {
    let route = __autumn_route_info_post_widgets();
    let body = route
        .api_doc
        .request_body
        .as_ref()
        .expect("Json<Vec<T>> request body must infer");
    assert!(matches!(body.kind, SchemaKind::Array(_)));
}

#[test]
fn valid_json_request_body_is_inferred() {
    let route = __autumn_route_info_create_validated_widget();
    let body = route
        .api_doc
        .request_body
        .as_ref()
        .expect("Valid<Json<T>> request body should be inferred");
    assert_eq!(body.name, "NewWidget");
    assert_eq!(body.kind, SchemaKind::Ref);
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
