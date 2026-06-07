//! Project typed Autumn endpoints as [Model Context Protocol][mcp] tools.
//!
//! Autumn already builds a route registry of [`ApiDoc`](crate::openapi::ApiDoc)
//! metadata — handler name, summary/description, and the request-body /
//! `Query` / path-param JSON Schemas — the same data that feeds
//! [`openapi`](crate::openapi). This module *projects* that registry into an
//! MCP server the way `openapi` projects it into an OpenAPI document, so an
//! existing JSON API becomes agent-callable with near-zero new code.
//!
//! What you write:
//!
//! ```ignore
//! #[get("/api/todos")]
//! #[api_doc(mcp, summary = "List todos")]
//! async fn list_todos() -> AutumnResult<Json<Vec<Todo>>> { /* ... */ }
//!
//! autumn_web::app()
//!     .routes(routes![list_todos])
//!     .mount_mcp("/mcp")        // serves a Streamable-HTTP MCP endpoint
//!     .run().await;
//! ```
//!
//! Key properties (issue #1117):
//!
//! * **Opt-in per endpoint** via `#[api_doc(mcp)]`; nothing is exposed
//!   implicitly. A whole-API hatch ([`AppBuilder::expose_all_as_mcp`]) is an
//!   explicit, separate call and still requires opt-in for mutating verbs.
//! * **No second schema.** Each tool's `inputSchema` is derived from the
//!   handler's typed `ApiDoc`, so it cannot drift from the handler.
//! * **Real pipeline.** `tools/call` dispatches through the exact same router
//!   an HTTP request hits, so `#[secured]`, authorization, tenancy, rate
//!   limits, and validation all apply identically.
//! * **Bearer auth reuse.** Agents present an API token via the existing
//!   [`RequireApiToken`](crate::auth::RequireApiToken) surface; the
//!   `Authorization` header is forwarded into the dispatched call.
//!
//! [`AppBuilder::expose_all_as_mcp`]: crate::app::AppBuilder::expose_all_as_mcp
//!
//! [mcp]: https://modelcontextprotocol.io

#![cfg(feature = "mcp")]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::openapi::{ApiDoc, schema_entry_to_value};

/// Protocol version advertised when a client does not request one.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Runtime MCP configuration carried from the [`AppBuilder`](crate::app::AppBuilder)
/// through router assembly.
#[derive(Clone, Debug)]
pub struct McpRuntime {
    /// Path the Streamable-HTTP endpoint is mounted at (e.g. `/mcp`).
    pub mount_path: String,
    /// When `true`, every eligible `GET` route is exposed without a
    /// per-endpoint tag (the whole-API hatch). Mutating verbs still require
    /// an explicit `#[api_doc(mcp)]` opt-in, and `#[api_doc(mcp = false)]`
    /// exclusions are always honored.
    pub expose_all: bool,
}

impl McpRuntime {
    /// Create a runtime config for a per-endpoint-opt-in MCP server.
    #[must_use]
    pub fn new(mount_path: impl Into<String>) -> Self {
        Self {
            mount_path: mount_path.into(),
            expose_all: false,
        }
    }
}

/// A single derived MCP tool plus the metadata needed to replay it as an
/// in-process HTTP request.
#[derive(Clone, Debug)]
struct McpTool {
    name: String,
    description: Option<String>,
    input_schema: Value,
    annotations: Value,
    // ── dispatch metadata ──
    method: String,
    /// Full route path with `{param}` placeholders.
    path_template: String,
    path_params: Vec<String>,
    has_body: bool,
    has_query: bool,
}

impl McpTool {
    /// The JSON object advertised in `tools/list`.
    fn descriptor(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), json!(self.name));
        if let Some(desc) = &self.description {
            obj.insert("description".into(), json!(desc));
        }
        obj.insert("inputSchema".into(), self.input_schema.clone());
        obj.insert("annotations".into(), self.annotations.clone());
        Value::Object(obj)
    }
}

/// The shared MCP server state attached to the endpoint handler. Holds the
/// derived tool catalog and a clone of the fully-assembled application router
/// to dispatch `tools/call` against.
pub struct McpServer {
    tools: Vec<McpTool>,
    by_name: HashMap<String, usize>,
    /// The real application router (state already applied) — the same path an
    /// HTTP request traverses. `tools/call` replays requests through it.
    dispatch: axum::Router,
    server_name: String,
    server_version: String,
}

/// Decide whether a route's `ApiDoc` should be projected as a tool.
///
/// Eligibility (JSON-out) gates everything: HTML/Maud routes have no response
/// schema and are silently ineligible. On top of that:
/// * `mcp_exclude` always wins.
/// * an explicit `mcp` opt-in always exposes (any verb).
/// * under the whole-API hatch, un-tagged `GET`s are auto-included but
///   mutating verbs are not.
fn should_expose(doc: &ApiDoc, expose_all: bool) -> bool {
    if doc.hidden || doc.mcp_exclude {
        return false;
    }
    // JSON-out only: a response schema is the structural signal that this is a
    // JSON endpoint rather than an HTML/Maud route.
    if doc.response.is_none() {
        return false;
    }
    if doc.mcp_tool {
        return true;
    }
    if expose_all {
        return is_read_only(doc.method);
    }
    false
}

/// `GET` (and `HEAD`) are read-only; everything else mutates.
fn is_read_only(method: &str) -> bool {
    matches!(method.to_ascii_uppercase().as_str(), "GET" | "HEAD")
}

/// MCP safety annotations derived purely from the HTTP verb.
fn annotations_for(method: &str, title: &str) -> Value {
    let upper = method.to_ascii_uppercase();
    let read_only = is_read_only(&upper);
    let mut obj = serde_json::Map::new();
    obj.insert("title".into(), json!(title));
    obj.insert("readOnlyHint".into(), json!(read_only));
    // DELETE is the destructive verb; flag it so agents/UIs can warn.
    if upper == "DELETE" {
        obj.insert("destructiveHint".into(), json!(true));
    }
    Value::Object(obj)
}

/// Build the `inputSchema` for a tool from the handler's typed contract.
///
/// Path params become required string properties, the `Query<T>` extractor
/// becomes a `query` object property, and the JSON request body becomes a
/// `body` property. Named component refs are inlined into `$defs` so the
/// schema is self-contained.
fn build_input_schema(doc: &ApiDoc, components: &serde_json::Map<String, Value>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    let mut defs = serde_json::Map::new();

    for param in doc.path_params {
        properties.insert((*param).to_owned(), json!({ "type": "string" }));
        required.push(json!(*param));
    }

    if let Some(query) = &doc.query_schema {
        let schema = rewrite_refs(schema_entry_to_value(query), components, &mut defs);
        properties.insert("query".to_owned(), schema);
    }

    if let Some(body) = &doc.request_body {
        let schema = rewrite_refs(schema_entry_to_value(body), components, &mut defs);
        properties.insert("body".to_owned(), schema);
        required.push(json!("body"));
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    if !defs.is_empty() {
        schema.insert("$defs".into(), Value::Object(defs));
    }
    Value::Object(schema)
}

/// Recursively rewrite `#/components/schemas/X` refs to local `#/$defs/X`
/// refs, pulling each referenced component (resolved from `components`) into
/// `defs` so the resulting schema stands alone.
fn rewrite_refs(
    value: Value,
    components: &serde_json::Map<String, Value>,
    defs: &mut serde_json::Map<String, Value>,
) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                let name = name.to_owned();
                let local = format!("#/$defs/{name}");
                if !defs.contains_key(&name) {
                    // Insert a placeholder first to break ref cycles, then
                    // resolve the real component schema (if registered).
                    defs.insert(name.clone(), Value::Null);
                    let resolved = components
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object", "title": name.clone() }));
                    let resolved = rewrite_refs(resolved, components, defs);
                    defs.insert(name, resolved);
                }
                return json!({ "$ref": local });
            }
            let rewritten: serde_json::Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, rewrite_refs(v, components, defs)))
                .collect();
            Value::Object(rewritten)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| rewrite_refs(v, components, defs))
                .collect(),
        ),
        other => other,
    }
}

/// Derive the tool catalog from collected route docs.
///
/// Emits a build-time `tracing::warn` for any endpoint that opts into MCP but
/// is ineligible (e.g. an HTML/Maud route with no JSON response schema), so it
/// is a logged note rather than a runtime surprise.
#[must_use]
pub fn derive_tools(docs: &[ApiDoc], expose_all: bool) -> Vec<McpToolInfo> {
    // Reuse the OpenAPI generator to resolve component schemas exactly once,
    // so tool input schemas share the handler's typed contract.
    let refs: Vec<&ApiDoc> = docs.iter().collect();
    let config = crate::openapi::OpenApiConfig::new("autumn-mcp", env!("CARGO_PKG_VERSION"));
    let spec = crate::openapi::generate_spec(&config, &refs);
    let components = spec
        .components
        .as_ref()
        .map(|c| serde_json::to_value(&c.schemas).unwrap_or(Value::Null))
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let mut tools = Vec::new();
    for doc in docs {
        // Surface the "opted in but ineligible" case as a build-time note.
        if (doc.mcp_tool || (expose_all && is_read_only(doc.method)))
            && doc.response.is_none()
            && !doc.mcp_exclude
            && !doc.hidden
        {
            tracing::warn!(
                operation_id = doc.operation_id,
                method = doc.method,
                path = doc.path,
                "skipping MCP exposure: endpoint has no JSON response schema \
                 (HTML/Maud routes are not eligible as MCP tools)"
            );
            continue;
        }
        if !should_expose(doc, expose_all) {
            continue;
        }
        let title = doc.summary.unwrap_or(doc.operation_id);
        tools.push(McpToolInfo {
            name: doc.operation_id.to_owned(),
            description: doc.description.or(doc.summary).map(str::to_owned),
            input_schema: build_input_schema(doc, &components),
            annotations: annotations_for(doc.method, title),
            method: doc.method.to_owned(),
            path_template: doc.path.to_owned(),
            path_params: doc.path_params.iter().map(|p| (*p).to_owned()).collect(),
            has_body: doc.request_body.is_some(),
            has_query: doc.query_schema.is_some(),
        });
    }
    tools
}

/// Public, transport-agnostic description of a derived tool. Returned by
/// [`derive_tools`] and consumed by [`McpServer::new`].
#[derive(Clone, Debug)]
pub struct McpToolInfo {
    name: String,
    description: Option<String>,
    input_schema: Value,
    annotations: Value,
    method: String,
    path_template: String,
    path_params: Vec<String>,
    has_body: bool,
    has_query: bool,
}

impl McpServer {
    /// Assemble the server state from derived tools and a dispatch router.
    #[must_use]
    pub fn new(tools: Vec<McpToolInfo>, dispatch: axum::Router) -> Self {
        let tools: Vec<McpTool> = tools
            .into_iter()
            .map(|t| McpTool {
                name: t.name,
                description: t.description,
                input_schema: t.input_schema,
                annotations: t.annotations,
                method: t.method,
                path_template: t.path_template,
                path_params: t.path_params,
                has_body: t.has_body,
                has_query: t.has_query,
            })
            .collect();
        let by_name = tools
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        Self {
            tools,
            by_name,
            dispatch,
            server_name: "autumn-mcp".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Build an axum sub-router serving the MCP endpoint at `mount_path`.
///
/// `dispatch` must be the fully-assembled application router (state applied)
/// so `tools/call` traverses the real handler pipeline.
pub fn build_mcp_router(
    mount_path: &str,
    tools: Vec<McpToolInfo>,
    dispatch: axum::Router,
) -> axum::Router<crate::state::AppState> {
    let server = Arc::new(McpServer::new(tools, dispatch));
    tracing::debug!(
        path = mount_path,
        tools = server.tools.len(),
        "Mounted MCP endpoint"
    );
    axum::Router::<crate::state::AppState>::new()
        .route(mount_path, axum::routing::post(serve_mcp))
        .route(mount_path, axum::routing::get(serve_mcp_get))
        .layer(axum::extract::Extension(server))
}

/// MCP over Streamable HTTP: GET opens a server-initiated stream. This buffered
/// v1 has nothing to stream, so we politely decline (SSE is tracked in #1118).
async fn serve_mcp_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        "MCP server-initiated streaming is not supported (POST JSON-RPC only)",
    )
        .into_response()
}

/// The Streamable-HTTP POST handler: parses one JSON-RPC message (or a batch)
/// and responds with `application/json`.
async fn serve_mcp(
    axum::extract::Extension(server): axum::extract::Extension<Arc<McpServer>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return json_response(&parse_error(&e.to_string()));
        }
    };

    match parsed {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for msg in batch {
                if let Some(resp) = handle_message(&server, &headers, msg).await {
                    out.push(resp);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                json_response(&Value::Array(out))
            }
        }
        // A notification (no `id`) yields `None` → an empty 202 per the spec.
        msg => handle_message(&server, &headers, msg).await.map_or_else(
            || StatusCode::ACCEPTED.into_response(),
            |v| json_response(&v),
        ),
    }
}

/// Handle a single JSON-RPC message. Returns `None` for notifications.
async fn handle_message(server: &McpServer, headers: &HeaderMap, msg: Value) -> Option<Value> {
    // Notifications have no `id` member and never get a response.
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(initialize_result(server, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list_result(server)),
        "tools/call" => return Some(tools_call(server, headers, id, &params).await),
        other => Err((-32601, format!("method not found: {other}"))),
    };

    Some(match result {
        Ok(value) => success(id, value),
        Err((code, message)) => error(id, code, &message),
    })
}

fn initialize_result(server: &McpServer, params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": server.server_name,
            "version": server.server_version,
        },
    })
}

fn tools_list_result(server: &McpServer) -> Value {
    let tools: Vec<Value> = server.tools.iter().map(McpTool::descriptor).collect();
    json!({ "tools": tools })
}

/// Dispatch a `tools/call` through the real router and shape the response as
/// an MCP tool result.
async fn tools_call(server: &McpServer, headers: &HeaderMap, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Some(&idx) = server.by_name.get(name) else {
        return error(id, -32602, &format!("unknown tool: {name}"));
    };
    let tool = &server.tools[idx];

    let request = match build_request(tool, headers, &arguments) {
        Ok(req) => req,
        Err(message) => return error(id, -32602, &message),
    };

    let response = match server.dispatch.clone().oneshot(request).await {
        Ok(resp) => resp,
        Err(e) => {
            return success(id, tool_error(&format!("dispatch failed: {e}")));
        }
    };

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();

    if status.is_success() {
        success(id, tool_ok(&text))
    } else {
        success(
            id,
            tool_error(&format!(
                "handler returned HTTP {}: {text}",
                status.as_u16()
            )),
        )
    }
}

/// Reconstruct an in-process HTTP request from a tool call's arguments.
fn build_request(
    tool: &McpTool,
    headers: &HeaderMap,
    arguments: &Value,
) -> Result<axum::http::Request<Body>, String> {
    // Fill the path template from top-level string-ish arguments.
    let mut path = tool.path_template.clone();
    for param in &tool.path_params {
        let raw = arguments
            .get(param)
            .ok_or_else(|| format!("missing required path parameter `{param}`"))?;
        let value = match raw {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let encoded = percent_encode_segment(&value);
        path = replace_path_param(&path, param, &encoded);
    }

    // Build the query string from the `query` object, if any.
    if tool.has_query
        && let Some(Value::Object(map)) = arguments.get("query")
    {
        let pairs: Vec<(String, String)> = map
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), value)
            })
            .collect();
        if !pairs.is_empty() {
            let qs = serde_urlencoded::to_string(&pairs)
                .map_err(|e| format!("invalid query arguments: {e}"))?;
            path = format!("{path}?{qs}");
        }
    }

    let mut builder = axum::http::Request::builder()
        .method(tool.method.as_str())
        .uri(&path);

    // Forward the agent's bearer credential so RequireApiToken / #[secured]
    // and friends see the same principal an HTTP caller would present.
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        builder = builder.header(header::AUTHORIZATION, auth);
    }

    let body = if tool.has_body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        let payload = arguments.get("body").cloned().unwrap_or_else(|| json!({}));
        Body::from(serde_json::to_vec(&payload).unwrap_or_default())
    } else {
        Body::empty()
    };

    builder
        .body(body)
        .map_err(|e| format!("invalid request: {e}"))
}

/// Replace a single `{name}` / `{name:regex}` capture in a path template.
fn replace_path_param(path: &str, name: &str, value: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let inner = &after[..end];
            let capture = inner.split(':').next().unwrap_or(inner).trim();
            if capture == name {
                out.push_str(value);
            } else {
                out.push('{');
                out.push_str(inner);
                out.push('}');
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Percent-encode a single path segment value.
fn percent_encode_segment(value: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    // Encode characters that are unsafe inside a path segment.
    const SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'/')
        .add(b'?')
        .add(b'#')
        .add(b'%')
        .add(b'&');
    utf8_percent_encode(value, SEGMENT).to_string()
}

// ── MCP tool-result helpers ───────────────────────────────────────

fn tool_ok(text: &str) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": false,
    })
}

fn tool_error(text: &str) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": true,
    })
}

// ── JSON-RPC envelope helpers ─────────────────────────────────────

fn success(id: Value, result: Value) -> Value {
    // Build by hand so `id`/`result` are moved (not borrowed via `json!`).
    let mut obj = serde_json::Map::new();
    obj.insert("jsonrpc".into(), json!("2.0"));
    obj.insert("id".into(), id);
    obj.insert("result".into(), result);
    Value::Object(obj)
}

fn error(id: Value, code: i64, message: &str) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("jsonrpc".into(), json!("2.0"));
    obj.insert("id".into(), id);
    obj.insert("error".into(), json!({ "code": code, "message": message }));
    Value::Object(obj)
}

fn parse_error(message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32700, "message": format!("parse error: {message}") },
    })
}

fn json_response(value: &Value) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openapi::{SchemaEntry, SchemaKind};

    fn doc(method: &'static str, path: &'static str, op: &'static str) -> ApiDoc {
        ApiDoc {
            method,
            path,
            operation_id: op,
            success_status: 200,
            response: Some(SchemaEntry {
                name: "Todo",
                kind: SchemaKind::Ref,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn opt_in_required_without_hatch() {
        let mut d = doc("GET", "/a", "a");
        assert!(!should_expose(&d, false), "no opt-in => not exposed");
        d.mcp_tool = true;
        assert!(should_expose(&d, false));
    }

    #[test]
    fn exclude_always_wins() {
        let mut d = doc("GET", "/a", "a");
        d.mcp_tool = true;
        d.mcp_exclude = true;
        assert!(!should_expose(&d, false));
        assert!(!should_expose(&d, true));
    }

    #[test]
    fn hatch_includes_reads_excludes_unopted_writes() {
        let read = doc("GET", "/a", "a");
        let write = doc("POST", "/a", "b");
        assert!(should_expose(&read, true));
        assert!(!should_expose(&write, true), "mutating verb needs opt-in");
    }

    #[test]
    fn hatch_still_allows_opted_in_writes() {
        let mut write = doc("POST", "/a", "b");
        write.mcp_tool = true;
        assert!(should_expose(&write, true));
    }

    #[test]
    fn html_routes_are_ineligible() {
        let mut d = doc("GET", "/page", "page");
        d.response = None; // HTML/Maud route
        d.mcp_tool = true;
        assert!(!should_expose(&d, false));
    }

    #[test]
    fn annotations_track_method() {
        assert_eq!(annotations_for("GET", "t")["readOnlyHint"], json!(true));
        assert_eq!(annotations_for("POST", "t")["readOnlyHint"], json!(false));
        assert_eq!(
            annotations_for("DELETE", "t")["destructiveHint"],
            json!(true)
        );
        assert!(
            annotations_for("POST", "t")
                .get("destructiveHint")
                .is_none()
        );
    }

    #[test]
    fn input_schema_includes_path_param_and_body() {
        let mut d = doc("POST", "/users/{id}", "create");
        d.path_params = &["id"];
        d.request_body = Some(SchemaEntry {
            name: "NewUser",
            kind: SchemaKind::Ref,
        });
        let schema = build_input_schema(&d, &serde_json::Map::new());
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["id"].is_object());
        assert!(schema["properties"]["body"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("id")));
        assert!(required.contains(&json!("body")));
    }

    #[test]
    fn replace_path_param_handles_regex_captures() {
        assert_eq!(replace_path_param("/u/{id}", "id", "7"), "/u/7");
        assert_eq!(replace_path_param("/u/{id:[0-9]+}", "id", "7"), "/u/7");
        assert_eq!(
            replace_path_param("/u/{id}/p/{pid}", "pid", "9"),
            "/u/{id}/p/9"
        );
    }
}
