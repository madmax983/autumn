// OpenAPI/JSON/JSON-schema all appear frequently here and are legitimate
// acronyms, so silence clippy::doc_markdown rather than wrapping every
// mention in backticks.
#![allow(clippy::doc_markdown)]

//! OpenAPI (Swagger) specification auto-generation.
//!
//! Autumn automatically infers an OpenAPI 3.0 document from your
//! annotated routes ([`get`](crate::get), [`post`](crate::post), etc.),
//! their path parameters, and the extractor / response types in each
//! handler signature. The generated spec is served at `/v3/api-docs` and
//! a Swagger UI is served at `/swagger-ui` when the feature is enabled.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! #[get("/hello")]
//! async fn hello() -> &'static str { "hi" }
//!
//! # #[autumn_web::main]
//! # async fn main() {
//! autumn_web::app()
//!     .routes(routes![hello])
//!     .openapi(autumn_web::openapi::OpenApiConfig::new("My API", "1.0.0"))
//!     .run()
//!     .await;
//! # }
//! ```
//!
//! With `.openapi(...)` enabled, the following endpoints are mounted:
//! * `GET /v3/api-docs` — serves the generated `openapi.json`.
//! * `GET /swagger-ui` — serves a Swagger UI HTML page loading the JSON
//!   above.
//!
//! # Enriching the auto-generated docs
//!
//! Decorate handlers with [`#[api_doc(...)]`](crate::api_doc) to override
//! or add documentation fields that cannot be inferred from the signature
//! (summaries, descriptions, tags, custom status codes, etc.):
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! #[get("/users/{id}")]
//! #[api_doc(summary = "Fetch a user by id", tag = "users")]
//! async fn get_user(_id: Path<i32>) -> &'static str { "user" }
//! ```
//!
//! # Custom schemas
//!
//! Types that need rich schemas (beyond the generic "object" fallback)
//! implement the [`OpenApiSchema`] trait and are registered with
//! [`OpenApiConfig::register_schema`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────
// Public metadata attached to each Route
// ──────────────────────────────────────────────────────────────────

/// OpenAPI metadata emitted alongside every annotated route.
///
/// Populated by the route macros ([`get`](crate::get),
/// [`post`](crate::post), etc.) from the handler's path, signature, and
/// any [`#[api_doc(...)]`](crate::api_doc) overrides.
#[derive(Clone, Debug, Default)]
pub struct ApiDoc {
    /// HTTP method as an uppercase string (e.g. `"GET"`).
    pub method: &'static str,
    /// Raw route path with `{param}` placeholders (e.g. `"/users/{id}"`).
    pub path: &'static str,
    /// Handler function name — used as the default `operationId`.
    pub operation_id: &'static str,
    /// Short human-readable summary (from `#[api_doc(summary = ...)]`).
    pub summary: Option<&'static str>,
    /// Longer free-form description.
    pub description: Option<&'static str>,
    /// Grouping tags. Defaults to the first path segment when unset.
    pub tags: &'static [&'static str],
    /// Path parameter names extracted from the URL template.
    ///
    /// Built at compile time from `{...}` segments in the route path.
    pub path_params: &'static [&'static str],
    /// Optional schema for the request body (typically the inner type of
    /// a `Json<T>` extractor).
    pub request_body: Option<SchemaEntry>,
    /// Optional schema for the success response (typically the inner type
    /// of a `Json<T>` return value).
    pub response: Option<SchemaEntry>,
    /// Success HTTP status code, defaults to `200`.
    pub success_status: u16,
    /// When `true`, the route is excluded from the generated spec.
    pub hidden: bool,
    /// Optional runtime hook that lets a handler register any extra
    /// component schemas with the generator.
    pub register_schemas: Option<fn(&mut SchemaRegistry)>,
}

/// Reference to a schema definition, produced by the route macros.
#[derive(Clone, Debug)]
pub struct SchemaEntry {
    /// Short human-readable type name (used as `#/components/schemas/Name`).
    pub name: &'static str,
    /// Whether this is a primitive JSON type (string/number/bool/array) as
    /// opposed to a named object ref.
    pub kind: SchemaKind,
}

/// Classifier for how a type should appear in the spec.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SchemaKind {
    /// Refers to a named component schema.
    Ref,
    /// A primitive JSON type inlined at the reference site.
    Primitive(&'static str),
}

// ──────────────────────────────────────────────────────────────────
// Configuration — users opt into OpenAPI generation explicitly.
// ──────────────────────────────────────────────────────────────────

/// User-facing configuration for OpenAPI generation.
///
/// Passed to [`AppBuilder::openapi`](crate::app::AppBuilder::openapi)
/// to enable spec generation and mount the documentation endpoints.
#[derive(Clone)]
pub struct OpenApiConfig {
    /// API title that appears in the Swagger UI header.
    pub title: String,
    /// API version (e.g. `"1.0.0"`).
    pub version: String,
    /// Optional free-form API description (Markdown permitted in UI).
    pub description: Option<String>,
    /// Path serving the raw `openapi.json`. Defaults to `/v3/api-docs`.
    pub openapi_json_path: String,
    /// Path serving the Swagger UI HTML. Defaults to `/swagger-ui`. Set
    /// to `None` to disable the UI while still exposing the JSON.
    pub swagger_ui_path: Option<String>,
    /// User-registered component schemas keyed by schema name.
    pub additional_schemas: BTreeMap<String, serde_json::Value>,
}

impl OpenApiConfig {
    /// Create a new config with the required `title` and `version`.
    #[must_use]
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: None,
            openapi_json_path: "/v3/api-docs".to_owned(),
            swagger_ui_path: Some("/swagger-ui".to_owned()),
            additional_schemas: BTreeMap::new(),
        }
    }

    /// Set a free-form API description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Override the path serving `openapi.json`.
    #[must_use]
    pub fn openapi_json_path(mut self, path: impl Into<String>) -> Self {
        self.openapi_json_path = path.into();
        self
    }

    /// Override the Swagger UI path (or `None` to disable it).
    #[must_use]
    pub fn swagger_ui_path(mut self, path: Option<String>) -> Self {
        self.swagger_ui_path = path;
        self
    }

    /// Register a custom component schema. Useful when a handler's
    /// payload type does not implement [`OpenApiSchema`].
    #[must_use]
    pub fn register_schema(mut self, name: impl Into<String>, schema: serde_json::Value) -> Self {
        self.additional_schemas.insert(name.into(), schema);
        self
    }
}

// ──────────────────────────────────────────────────────────────────
// Schema trait + primitive impls
// ──────────────────────────────────────────────────────────────────

/// Describes a type's JSON schema for OpenAPI generation.
///
/// Provide a manual implementation for complex types to expose rich
/// schemas in the generated spec. A blanket default is not provided —
/// routes whose types do not implement this trait simply emit a generic
/// `object` placeholder referring to the type name.
pub trait OpenApiSchema {
    /// Component schema name (appears under `#/components/schemas/`).
    fn schema_name() -> &'static str;

    /// Produce the JSON schema for this type.
    fn schema() -> serde_json::Value;
}

macro_rules! impl_primitive_schema {
    ($ty:ty, $name:literal, $json:literal) => {
        impl OpenApiSchema for $ty {
            fn schema_name() -> &'static str {
                $name
            }
            fn schema() -> serde_json::Value {
                serde_json::json!({ "type": $json })
            }
        }
    };
}

impl_primitive_schema!(bool, "boolean", "boolean");
impl_primitive_schema!(String, "string", "string");
impl_primitive_schema!(&'static str, "string", "string");
impl_primitive_schema!(i8, "integer", "integer");
impl_primitive_schema!(i16, "integer", "integer");
impl_primitive_schema!(i32, "integer", "integer");
impl_primitive_schema!(i64, "integer", "integer");
impl_primitive_schema!(u8, "integer", "integer");
impl_primitive_schema!(u16, "integer", "integer");
impl_primitive_schema!(u32, "integer", "integer");
impl_primitive_schema!(u64, "integer", "integer");
impl_primitive_schema!(f32, "number", "number");
impl_primitive_schema!(f64, "number", "number");
impl_primitive_schema!(serde_json::Value, "object", "object");

// ──────────────────────────────────────────────────────────────────
// Runtime registry of component schemas populated while building the spec.
// ──────────────────────────────────────────────────────────────────

/// Accumulates component schemas while a spec is being built.
#[derive(Default)]
pub struct SchemaRegistry {
    schemas: BTreeMap<String, serde_json::Value>,
}

impl SchemaRegistry {
    /// Register a type via its [`OpenApiSchema`] implementation. A
    /// duplicate insertion is a no-op (the existing entry wins).
    pub fn register<T: OpenApiSchema>(&mut self) {
        let name = T::schema_name().to_owned();
        self.schemas.entry(name).or_insert_with(T::schema);
    }

    /// Insert a raw pre-built schema by name.
    pub fn insert(&mut self, name: impl Into<String>, schema: serde_json::Value) {
        self.schemas.insert(name.into(), schema);
    }

    /// Drain the collected schemas, consuming the registry.
    #[must_use]
    pub fn into_map(self) -> BTreeMap<String, serde_json::Value> {
        self.schemas
    }

    /// Peek at the collected schemas without consuming the registry.
    #[must_use]
    pub const fn schemas(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.schemas
    }
}

// ──────────────────────────────────────────────────────────────────
// Serializable OpenAPI 3.0 document types.
//
// Only the fields Autumn actually populates are modelled — unused
// OpenAPI keys (callbacks, links, discriminators…) are intentionally
// omitted so the generated JSON stays clean.
// ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenApiSpec {
    pub openapi: String,
    pub info: Info,
    pub paths: BTreeMap<String, PathItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    pub title: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct PathItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub put: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Operation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Operation {
    #[serde(rename = "operationId")]
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(rename = "requestBody", skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,
    pub responses: BTreeMap<String, Response>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "in")]
    pub location: String,
    pub required: bool,
    pub schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestBody {
    pub required: bool,
    pub content: BTreeMap<String, MediaType>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub description: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub content: BTreeMap<String, MediaType>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MediaType {
    pub schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Components {
    pub schemas: BTreeMap<String, serde_json::Value>,
}

// ──────────────────────────────────────────────────────────────────
// Spec generator
// ──────────────────────────────────────────────────────────────────

/// Build an [`OpenApiSpec`] from a collection of routes and user config.
///
/// This is the core of the auto-generation: every route's [`ApiDoc`] is
/// translated into an [`Operation`] under the matching [`PathItem`].
#[must_use]
pub fn generate_spec(config: &OpenApiConfig, routes: &[&ApiDoc]) -> OpenApiSpec {
    let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
    let mut registry = SchemaRegistry::default();

    for (name, schema) in &config.additional_schemas {
        registry.insert(name.clone(), schema.clone());
    }

    // Collect every named schema reference produced by any operation so
    // we can back-fill component entries for types the user didn't
    // explicitly register. Without this, auto-inferred `Json<MyDto>`
    // payloads would emit `$ref`s pointing at nonexistent component
    // schemas — an invalid OpenAPI document.
    let mut referenced_names: std::collections::BTreeSet<&'static str> =
        std::collections::BTreeSet::new();

    for api_doc in routes {
        if api_doc.hidden {
            continue;
        }
        if let Some(register) = api_doc.register_schemas {
            (register)(&mut registry);
        }

        if let Some(entry) = &api_doc.request_body
            && entry.kind == SchemaKind::Ref
        {
            referenced_names.insert(entry.name);
        }
        if let Some(entry) = &api_doc.response
            && entry.kind == SchemaKind::Ref
        {
            referenced_names.insert(entry.name);
        }

        let operation = operation_for(api_doc);
        let entry = paths.entry(api_doc.path.to_owned()).or_default();
        match api_doc.method {
            "GET" => entry.get = Some(operation),
            "POST" => entry.post = Some(operation),
            "PUT" => entry.put = Some(operation),
            "DELETE" => entry.delete = Some(operation),
            "PATCH" => entry.patch = Some(operation),
            // Unknown methods are silently skipped; Autumn's route macros
            // only emit the five verbs above today.
            _ => {}
        }
    }

    // Back-fill a minimal `{"type": "object", "title": "X"}` schema for
    // every referenced name the user didn't already register. Types that
    // implement OpenApiSchema can be registered explicitly via
    // OpenApiConfig::register_schema to replace the placeholder.
    for name in referenced_names {
        if !registry.schemas().contains_key(name) {
            registry.insert(
                name,
                serde_json::json!({
                    "type": "object",
                    "title": name,
                }),
            );
        }
    }

    let components_map = registry.into_map();
    let components = if components_map.is_empty() {
        None
    } else {
        Some(Components {
            schemas: components_map,
        })
    };

    OpenApiSpec {
        openapi: "3.0.3".to_owned(),
        info: Info {
            title: config.title.clone(),
            version: config.version.clone(),
            description: config.description.clone(),
        },
        paths,
        components,
    }
}

fn operation_for(api_doc: &ApiDoc) -> Operation {
    let tags = if api_doc.tags.is_empty() {
        default_tag(api_doc.path)
            .map(|t| vec![t.to_owned()])
            .unwrap_or_default()
    } else {
        api_doc.tags.iter().map(|s| (*s).to_owned()).collect()
    };

    let parameters = api_doc
        .path_params
        .iter()
        .map(|name| Parameter {
            name: (*name).to_owned(),
            location: "path".to_owned(),
            required: true,
            schema: serde_json::json!({ "type": "string" }),
        })
        .collect();

    let request_body = api_doc.request_body.as_ref().map(|entry| RequestBody {
        required: true,
        content: std::iter::once((
            "application/json".to_owned(),
            MediaType {
                schema: schema_value_for(entry),
            },
        ))
        .collect(),
    });

    let mut responses: BTreeMap<String, Response> = BTreeMap::new();
    let status = if api_doc.success_status == 0 {
        200
    } else {
        api_doc.success_status
    };
    let response_content = api_doc
        .response
        .as_ref()
        .map(|entry| {
            let mut content = BTreeMap::new();
            content.insert(
                "application/json".to_owned(),
                MediaType {
                    schema: schema_value_for(entry),
                },
            );
            content
        })
        .unwrap_or_default();
    responses.insert(
        status.to_string(),
        Response {
            description: status_description(status).to_owned(),
            content: response_content,
        },
    );

    Operation {
        operation_id: api_doc.operation_id.to_owned(),
        summary: api_doc.summary.map(str::to_owned),
        description: api_doc.description.map(str::to_owned),
        tags,
        parameters,
        request_body,
        responses,
    }
}

fn schema_value_for(entry: &SchemaEntry) -> serde_json::Value {
    match entry.kind {
        SchemaKind::Primitive(json_type) => serde_json::json!({ "type": json_type }),
        SchemaKind::Ref => {
            serde_json::json!({ "$ref": format!("#/components/schemas/{}", entry.name) })
        }
    }
}

fn default_tag(path: &str) -> Option<&str> {
    path.trim_start_matches('/')
        .split('/')
        .find(|seg| !seg.is_empty() && !seg.starts_with('{'))
}

const fn status_description(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        _ => "Response",
    }
}

// ──────────────────────────────────────────────────────────────────
// Swagger UI HTML
// ──────────────────────────────────────────────────────────────────

/// Minimal Swagger UI bootstrap HTML that loads the generated JSON.
///
/// Uses the public unpkg CDN for Swagger UI assets so no static files
/// need to be embedded in the framework binary.
#[must_use]
pub fn swagger_ui_html(spec_url: &str, title: &str) -> String {
    let title = html_escape(title);
    let spec_url = html_escape(spec_url);
    // Assembled as a single String (not a format!()) to avoid conflicts
    // between Rust's raw-string `#` delimiters and the CSS selector
    // `#swagger-ui` in the embedded JS.
    let mut out = String::with_capacity(1024);
    out.push_str("<!DOCTYPE html>\n");
    out.push_str("<html lang=\"en\">\n");
    out.push_str("  <head>\n");
    out.push_str("    <meta charset=\"utf-8\" />\n");
    out.push_str("    <title>");
    out.push_str(&title);
    out.push_str("</title>\n");
    out.push_str(
        "    <link rel=\"stylesheet\" \
         href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\" />\n",
    );
    out.push_str("  </head>\n");
    out.push_str("  <body>\n");
    out.push_str("    <div id=\"swagger-ui\"></div>\n");
    out.push_str(
        "    <script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\" \
         charset=\"UTF-8\"></script>\n",
    );
    out.push_str("    <script>\n");
    out.push_str("      window.onload = function() {\n");
    out.push_str("        window.ui = SwaggerUIBundle({\n");
    out.push_str("          url: \"");
    out.push_str(&spec_url);
    out.push_str("\",\n");
    out.push_str("          dom_id: \"#swagger-ui\",\n");
    out.push_str("          deepLinking: true\n");
    out.push_str("        });\n");
    out.push_str("      };\n");
    out.push_str("    </script>\n");
    out.push_str("  </body>\n");
    out.push_str("</html>\n");
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ──────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc() -> ApiDoc {
        ApiDoc {
            method: "GET",
            path: "/users/{id}",
            operation_id: "get_user",
            summary: Some("Fetch a user"),
            description: None,
            tags: &[],
            path_params: &["id"],
            request_body: None,
            response: None,
            success_status: 200,
            hidden: false,
            register_schemas: None,
        }
    }

    #[test]
    fn generate_spec_builds_path_with_parameters() {
        let doc = make_doc();
        let config = OpenApiConfig::new("Demo", "1.0.0");
        let spec = generate_spec(&config, &[&doc]);

        assert_eq!(spec.openapi, "3.0.3");
        assert_eq!(spec.info.title, "Demo");
        assert!(spec.paths.contains_key("/users/{id}"));

        let op = spec.paths["/users/{id}"].get.as_ref().unwrap();
        assert_eq!(op.operation_id, "get_user");
        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].name, "id");
        assert_eq!(op.parameters[0].location, "path");
        assert_eq!(op.tags, vec!["users".to_owned()]);
    }

    #[test]
    fn generate_spec_skips_hidden_routes() {
        let mut doc = make_doc();
        doc.hidden = true;
        let config = OpenApiConfig::new("Demo", "1.0.0");
        let spec = generate_spec(&config, &[&doc]);
        assert!(spec.paths.is_empty());
    }

    #[test]
    fn generate_spec_writes_request_body_ref() {
        let mut doc = make_doc();
        doc.method = "POST";
        doc.path = "/users";
        doc.operation_id = "create_user";
        doc.path_params = &[];
        doc.request_body = Some(SchemaEntry {
            name: "CreateUser",
            kind: SchemaKind::Ref,
        });
        doc.success_status = 201;

        let config = OpenApiConfig::new("Demo", "1.0.0");
        let spec = generate_spec(&config, &[&doc]);
        let op = spec.paths["/users"].post.as_ref().unwrap();
        let body = op.request_body.as_ref().unwrap();
        assert!(body.required);
        let media = body.content.get("application/json").unwrap();
        assert_eq!(
            media.schema,
            serde_json::json!({ "$ref": "#/components/schemas/CreateUser" }),
        );
        assert!(op.responses.contains_key("201"));
    }

    #[test]
    fn generate_spec_inlines_primitive_response() {
        let mut doc = make_doc();
        doc.response = Some(SchemaEntry {
            name: "string",
            kind: SchemaKind::Primitive("string"),
        });
        let config = OpenApiConfig::new("Demo", "1.0.0");
        let spec = generate_spec(&config, &[&doc]);
        let op = spec.paths["/users/{id}"].get.as_ref().unwrap();
        let media = op.responses["200"].content.get("application/json").unwrap();
        assert_eq!(media.schema, serde_json::json!({ "type": "string" }));
    }

    #[test]
    fn generate_spec_includes_additional_schemas() {
        let doc = make_doc();
        let config = OpenApiConfig::new("Demo", "1.0.0")
            .register_schema("Foo", serde_json::json!({ "type": "object" }));
        let spec = generate_spec(&config, &[&doc]);
        let components = spec.components.unwrap();
        assert!(components.schemas.contains_key("Foo"));
    }

    #[test]
    fn generate_spec_back_fills_unregistered_ref_schemas() {
        // A Json<CreateUser> handler emits a `$ref` with no component
        // schema registered. The generator must back-fill a placeholder
        // schema so the resulting OpenAPI document is valid.
        let mut doc = make_doc();
        doc.method = "POST";
        doc.path = "/users";
        doc.path_params = &[];
        doc.request_body = Some(SchemaEntry {
            name: "CreateUser",
            kind: SchemaKind::Ref,
        });
        doc.response = Some(SchemaEntry {
            name: "User",
            kind: SchemaKind::Ref,
        });

        let config = OpenApiConfig::new("Demo", "1.0.0");
        let spec = generate_spec(&config, &[&doc]);
        let components = spec.components.expect("components must be emitted");
        let create = components
            .schemas
            .get("CreateUser")
            .expect("CreateUser should be back-filled");
        let user = components
            .schemas
            .get("User")
            .expect("User should be back-filled");
        assert_eq!(create["type"], "object");
        assert_eq!(create["title"], "CreateUser");
        assert_eq!(user["type"], "object");
        assert_eq!(user["title"], "User");
    }

    #[test]
    fn generate_spec_preserves_user_registered_schemas_over_backfill() {
        let mut doc = make_doc();
        doc.response = Some(SchemaEntry {
            name: "User",
            kind: SchemaKind::Ref,
        });

        let user_schema = serde_json::json!({
            "type": "object",
            "properties": {"id": {"type": "integer"}},
        });
        let config =
            OpenApiConfig::new("Demo", "1.0.0").register_schema("User", user_schema.clone());
        let spec = generate_spec(&config, &[&doc]);
        let components = spec.components.unwrap();
        let stored = components.schemas.get("User").unwrap();
        assert_eq!(stored, &user_schema, "user schema must not be overwritten");
    }

    #[test]
    fn default_tag_picks_first_static_segment() {
        assert_eq!(default_tag("/users/{id}"), Some("users"));
        assert_eq!(default_tag("/api/v1/users"), Some("api"));
        assert_eq!(default_tag("/"), None);
        assert_eq!(default_tag("/{id}"), None);
    }

    #[test]
    fn schema_registry_deduplicates() {
        struct Foo;
        impl OpenApiSchema for Foo {
            fn schema_name() -> &'static str {
                "Foo"
            }
            fn schema() -> serde_json::Value {
                serde_json::json!({ "type": "object", "title": "Foo" })
            }
        }

        let mut registry = SchemaRegistry::default();
        registry.register::<Foo>();
        registry.register::<Foo>();
        assert_eq!(registry.schemas().len(), 1);
    }

    #[test]
    fn primitive_impls_cover_common_types() {
        assert_eq!(<String as OpenApiSchema>::schema_name(), "string");
        assert_eq!(<i32 as OpenApiSchema>::schema_name(), "integer");
        assert_eq!(<bool as OpenApiSchema>::schema_name(), "boolean");
        assert_eq!(<f64 as OpenApiSchema>::schema_name(), "number");
    }

    #[test]
    fn swagger_ui_html_embeds_spec_url() {
        let html = swagger_ui_html("/v3/api-docs", "My API");
        assert!(html.contains("/v3/api-docs"));
        assert!(html.contains("My API"));
    }

    #[test]
    fn swagger_ui_html_escapes_attributes() {
        let html = swagger_ui_html("/v3/api-docs?x=<y>", "A \"cool\" & fun API");
        assert!(html.contains("/v3/api-docs?x=&lt;y&gt;"));
        assert!(html.contains("A &quot;cool&quot; &amp; fun API"));
    }
}
