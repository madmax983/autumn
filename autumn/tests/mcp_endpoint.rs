//! Integration tests for the MCP (Model Context Protocol) tool surface
//! (issue #1117): expose typed endpoints as MCP tools so agents can call
//! the real, authenticated handler pipeline.
//!
//! Covers the acceptance criteria:
//! * Per-endpoint opt-in via `#[api_doc(mcp)]`; nothing exposed implicitly.
//! * `mount_mcp("/mcp")` serves a spec-compliant Streamable-HTTP endpoint
//!   handling `initialize`, `tools/list`, and `tools/call`.
//! * Tool `name`/`description`/`inputSchema` are derived from `ApiDoc`.
//! * `tools/call` dispatches through the real handler pipeline.
//! * Bearer-token auth (`RequireApiToken`) applies to agent calls.
//! * HTTP method → MCP safety annotations.
//! * JSON-only eligibility; HTML routes auto-excluded.
//! * Whole-API hatch (`expose_all_as_mcp`) still requires opt-in for
//!   mutating verbs and honors exclusions.

#![cfg(feature = "mcp")]

use std::sync::Arc;

use autumn_web::auth::{InMemoryApiTokenStore, RequireApiToken, issue_api_token};
use autumn_web::config::AutumnConfig;
use autumn_web::openapi::OpenApiConfig;
use autumn_web::prelude::*;
use autumn_web::test::{TestApp, TestClient};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
struct Todo {
    id: u32,
    title: String,
}

#[derive(Serialize, Deserialize)]
struct NewTodo {
    title: String,
}

#[get("/api/todos")]
#[api_doc(mcp, summary = "List all todos")]
async fn list_todos() -> AutumnResult<Json<Vec<Todo>>> {
    Ok(Json(vec![Todo {
        id: 1,
        title: "first".into(),
    }]))
}

#[get("/api/todos/{id}")]
#[api_doc(mcp, summary = "Fetch one todo")]
async fn get_todo(Path(id): Path<u32>) -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id,
        title: format!("todo {id}"),
    }))
}

#[post("/api/todos")]
#[api_doc(mcp, summary = "Create a todo")]
async fn create_todo(Json(body): Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id: 42,
        title: body.title,
    }))
}

#[delete("/api/todos/{id}")]
#[api_doc(mcp, summary = "Delete a todo")]
async fn delete_todo(Path(id): Path<u32>) -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id,
        title: "deleted".into(),
    }))
}

// A user route that already owns the `/mcp` path, used to exercise the
// mount-path collision preflight.
#[get("/mcp")]
async fn mcp_named_route() -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id: 0,
        title: "preexisting".into(),
    }))
}

// Opted-out JSON endpoint: eligible but explicitly excluded.
#[get("/api/secret")]
#[api_doc(mcp = false)]
async fn secret() -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id: 0,
        title: "secret".into(),
    }))
}

// Not opted in at all — must never be exposed.
#[get("/api/private")]
async fn private_route() -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id: 0,
        title: "private".into(),
    }))
}

// HTML route opted in: ineligible (no JSON response), auto-excluded.
#[get("/page")]
#[api_doc(mcp)]
async fn html_page() -> &'static str {
    "<h1>hi</h1>"
}

async fn rpc(client: &TestClient, body: serde_json::Value) -> serde_json::Value {
    let resp = client.post("/mcp").json(&body).send().await;
    resp.assert_ok();
    resp.json::<serde_json::Value>()
}

#[tokio::test]
async fn initialize_returns_server_info_and_tools_capability() {
    let client = TestApp::new()
        .routes(routes![list_todos, create_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {} }
        }),
    )
    .await;

    assert_eq!(out["jsonrpc"], "2.0");
    assert_eq!(out["id"], 1);
    assert!(out["result"]["capabilities"]["tools"].is_object());
    assert!(out["result"]["serverInfo"]["name"].is_string());
    assert_eq!(out["result"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn tools_list_derives_from_api_doc_and_honors_opt_in() {
    let client = TestApp::new()
        .routes(routes![
            list_todos,
            get_todo,
            create_todo,
            delete_todo,
            secret,
            private_route,
            html_page
        ])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;

    let tools = out["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    // Opted-in JSON endpoints are exposed.
    assert!(names.contains(&"list_todos"));
    assert!(names.contains(&"get_todo"));
    assert!(names.contains(&"create_todo"));
    assert!(names.contains(&"delete_todo"));
    // Explicitly excluded, not opted in, and HTML routes are absent.
    assert!(!names.contains(&"secret"), "mcp = false must exclude");
    assert!(!names.contains(&"private_route"), "no opt-in => excluded");
    assert!(!names.contains(&"html_page"), "HTML route is ineligible");

    // Description + inputSchema derived from ApiDoc.
    let create = tools.iter().find(|t| t["name"] == "create_todo").unwrap();
    assert_eq!(create["description"], "Create a todo");
    assert_eq!(create["inputSchema"]["type"], "object");
    assert!(
        create["inputSchema"]["properties"]["body"].is_object(),
        "request body becomes a `body` property"
    );

    let get = tools.iter().find(|t| t["name"] == "get_todo").unwrap();
    assert!(
        get["inputSchema"]["properties"]["id"].is_object(),
        "path param becomes a property"
    );
}

#[tokio::test]
async fn safety_annotations_track_http_method() {
    let client = TestApp::new()
        .routes(routes![list_todos, create_todo, delete_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    )
    .await;
    let tools = out["result"]["tools"].as_array().unwrap();
    let by = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap().clone();

    assert_eq!(by("list_todos")["annotations"]["readOnlyHint"], true);
    assert_eq!(by("create_todo")["annotations"]["readOnlyHint"], false);
    assert_eq!(by("delete_todo")["annotations"]["readOnlyHint"], false);
    assert_eq!(by("delete_todo")["annotations"]["destructiveHint"], true);
}

#[tokio::test]
async fn tools_call_dispatches_read_tool_through_real_pipeline() {
    let client = TestApp::new()
        .routes(routes![get_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params": {"name":"get_todo","arguments":{"id":"7"}}
        }),
    )
    .await;

    assert_ne!(out["result"]["isError"], true);
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    let payload: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["id"], 7);
    assert_eq!(payload["title"], "todo 7");
}

#[tokio::test]
async fn tools_call_dispatches_write_tool_with_body() {
    let client = TestApp::new()
        .routes(routes![create_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":5,"method":"tools/call",
            "params": {"name":"create_todo","arguments":{"body":{"title":"new one"}}}
        }),
    )
    .await;

    assert_ne!(out["result"]["isError"], true);
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    let payload: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["id"], 42);
    assert_eq!(payload["title"], "new one");
}

#[tokio::test]
async fn unknown_tool_returns_error_result() {
    let client = TestApp::new()
        .routes(routes![get_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":6,"method":"tools/call",
            "params": {"name":"nope","arguments":{}}
        }),
    )
    .await;

    // Either a JSON-RPC error or an isError tool result is acceptable.
    assert!(out.get("error").is_some() || out["result"]["isError"] == true);
}

// ── Bearer-token auth reuse + real-pipeline enforcement ───────────

#[get("/todos")]
#[api_doc(mcp, summary = "List protected todos")]
async fn secure_list() -> AutumnResult<Json<Vec<Todo>>> {
    Ok(Json(vec![Todo {
        id: 99,
        title: "secret-but-authorized".into(),
    }]))
}

#[tokio::test]
async fn tools_call_enforces_bearer_token_via_real_pipeline() {
    let store = Arc::new(InMemoryApiTokenStore::default());
    let token = issue_api_token(store.as_ref(), "agent:bot").await.unwrap();

    // A scoped group carries the real `RequireApiToken` layer *and* keeps
    // the route in the registry, so MCP derives the tool from its ApiDoc.
    let client = TestApp::new()
        .scoped(
            "/secure",
            RequireApiToken::new(store.clone()),
            routes![secure_list],
        )
        .mount_mcp("/mcp")
        .build();

    // Without a token → the real RequireApiToken layer rejects (isError).
    let denied = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params": {"name":"secure_list","arguments":{}}
        }),
    )
    .await;
    assert_eq!(denied["result"]["isError"], true, "no token must be denied");

    // With a valid bearer token forwarded by the agent → authorized.
    let resp = client
        .post("/mcp")
        .header("authorization", &format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc":"2.0","id":8,"method":"tools/call",
            "params": {"name":"secure_list","arguments":{}}
        }))
        .send()
        .await;
    resp.assert_ok();
    let out = resp.json::<serde_json::Value>();
    assert_ne!(out["result"]["isError"], true, "valid token must succeed");
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("secret-but-authorized"));
}

// ── Whole-API hatch ───────────────────────────────────────────────

#[tokio::test]
async fn expose_all_includes_reads_but_requires_opt_in_for_writes() {
    let client = TestApp::new()
        .routes(routes![
            list_todos,
            get_todo,
            create_todo,
            secret,
            private_route
        ])
        .expose_all_as_mcp()
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}),
    )
    .await;
    let tools = out["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    // GET endpoints auto-included under the hatch...
    assert!(
        names.contains(&"private_route"),
        "GET auto-included by hatch"
    );
    assert!(names.contains(&"list_todos"));
    assert!(names.contains(&"get_todo"));
    // create_todo carries explicit opt-in so it is still allowed.
    assert!(names.contains(&"create_todo"));
    // Explicit exclusion still wins, even under the hatch.
    assert!(
        !names.contains(&"secret"),
        "mcp = false honored under hatch"
    );
}

#[post("/api/bulk")]
async fn bulk_write(Json(_body): Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo {
        id: 7,
        title: "bulk".into(),
    }))
}

#[tokio::test]
async fn expose_all_excludes_unopted_mutating_verbs() {
    let client = TestApp::new()
        .routes(routes![list_todos, bulk_write])
        .expose_all_as_mcp()
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({"jsonrpc":"2.0","id":10,"method":"tools/list"}),
    )
    .await;
    let tools = out["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"list_todos"));
    assert!(
        !names.contains(&"bulk_write"),
        "mutating verb without opt-in is excluded even under the hatch"
    );
}

// ── JSON-RPC robustness + path validation ─────────────────────────

#[tokio::test]
async fn empty_batch_returns_invalid_request() {
    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .build();

    // An empty JSON-RPC batch is itself an Invalid Request (-32600).
    let out = rpc(&client, serde_json::json!([])).await;
    assert_eq!(out["error"]["code"], -32600);
}

#[tokio::test]
async fn malformed_request_returns_invalid_request() {
    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .build();

    // A bare scalar is not a valid JSON-RPC message.
    let out = rpc(&client, serde_json::json!(5)).await;
    assert_eq!(out["error"]["code"], -32600);
}

#[tokio::test]
#[should_panic(expected = "InvalidMcpPath")]
async fn mount_path_without_leading_slash_is_rejected() {
    // axum would otherwise panic at route time; we surface it as a
    // recoverable RouterBuildError instead.
    let _ = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("mcp")
        .build();
}

#[tokio::test]
#[should_panic(expected = "McpPathCollision")]
async fn mount_path_colliding_with_existing_route_is_rejected() {
    // An app route already owns `/mcp`; mounting the MCP endpoint there would
    // panic at merge time, so we surface a recoverable RouterBuildError.
    let _ = TestApp::new()
        .routes(routes![mcp_named_route])
        .mount_mcp("/mcp")
        .build();
}

#[tokio::test]
#[should_panic(expected = "McpPathCollision")]
async fn mount_path_colliding_with_framework_route_is_rejected() {
    // Mounting on a framework-owned GET path (the health probe) must also be
    // caught by the collision preflight, not just user routes.
    let _ = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/health")
        .build();
}

#[tokio::test]
#[should_panic(expected = "McpPathCollision")]
async fn mount_path_colliding_with_openapi_path_is_rejected() {
    // The OpenAPI JSON endpoint merges as a GET before the MCP router, so a
    // mount path that collides with it must be rejected up front.
    let _ = TestApp::new()
        .routes(routes![list_todos])
        .openapi(OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/mcp"))
        .mount_mcp("/mcp")
        .build();
}

#[tokio::test]
#[should_panic(expected = "McpPathCollision")]
async fn mount_path_under_nested_router_is_rejected() {
    // A raw router nested at `/api` owns every path under it and is mounted
    // before the MCP router, so mounting at `/api/mcp` would be shadowed by
    // (or panic against) the nest. The preflight must catch it like the
    // OpenAPI nest-collision check does.
    let nested = axum::Router::new().route("/thing", axum::routing::get(|| async { "ok" }));
    let _ = TestApp::new()
        .routes(routes![list_todos])
        .nest("/api", nested)
        .mount_mcp("/api/mcp")
        .build();
}

#[tokio::test]
#[should_panic(expected = "InvalidMcpPath")]
async fn dynamic_mount_path_is_rejected() {
    // A capture/catch-all mount path would shadow a whole path class; only a
    // single static endpoint is allowed.
    let _ = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/{tenant}/mcp")
        .build();
}

#[tokio::test]
async fn tools_call_requires_body_for_write_tools() {
    let client = TestApp::new()
        .routes(routes![create_todo])
        .mount_mcp("/mcp")
        .build();

    // `body` is advertised as required; omitting it is an invalid-params error
    // rather than a silently-dispatched empty body.
    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":11,"method":"tools/call",
            "params": {"name":"create_todo","arguments":{}}
        }),
    )
    .await;
    assert_eq!(out["error"]["code"], -32602);
}

#[tokio::test]
async fn input_schema_reuses_registered_openapi_component() {
    // A schema registered on the app's OpenApiConfig must flow into the tool's
    // inputSchema, identical to the served OpenAPI doc — not a placeholder.
    let client = TestApp::new()
        .routes(routes![create_todo])
        .openapi(OpenApiConfig::new("Demo", "1.0.0").register_schema(
            "NewTodo",
            serde_json::json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            }),
        ))
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({"jsonrpc":"2.0","id":12,"method":"tools/list"}),
    )
    .await;
    let tools = out["result"]["tools"].as_array().unwrap();
    let create = tools.iter().find(|t| t["name"] == "create_todo").unwrap();
    // The registered component (with its `title` property) is inlined into
    // `$defs`, rather than the `{type:object,title:NewTodo}` placeholder.
    assert_eq!(
        create["inputSchema"]["$defs"]["NewTodo"]["properties"]["title"]["type"],
        "string"
    );
}

#[tokio::test]
async fn non_object_arguments_are_rejected() {
    let client = TestApp::new()
        .routes(routes![get_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":13,"method":"tools/call",
            "params": {"name":"get_todo","arguments":"not-an-object"}
        }),
    )
    .await;
    assert_eq!(out["error"]["code"], -32602);
}

#[tokio::test]
async fn missing_jsonrpc_version_is_invalid_request() {
    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .build();

    // No `jsonrpc` member → not a valid JSON-RPC 2.0 request.
    let out = rpc(
        &client,
        serde_json::json!({ "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(out["error"]["code"], -32600);
}

#[tokio::test]
async fn malformed_batch_item_returns_error_not_silent_accept() {
    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .build();

    // `[5]` is not a valid notification; it must produce an error response.
    let out = rpc(&client, serde_json::json!([5])).await;
    assert_eq!(out[0]["error"]["code"], -32600);
}

#[tokio::test]
async fn disallowed_origin_is_forbidden() {
    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .build();

    // Test config has no CORS allowed_origins, so any browser Origin is rejected
    // (DNS-rebinding protection). Non-browser agents (no Origin) are unaffected.
    let resp = client
        .post("/mcp")
        .header("origin", "https://evil.example")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await;
    resp.assert_status(403);
}

#[tokio::test]
async fn allowlisted_origin_gets_cors_grant_on_response() {
    // The endpoint sits outside the global CORS layer, so the actual JSON-RPC
    // response (not just the OPTIONS preflight) must carry
    // `Access-Control-Allow-Origin` or a browser will block reading it.
    let mut config = AutumnConfig::default();
    config.cors.allowed_origins = vec!["https://app.example".to_owned()];
    let client = TestApp::new()
        .routes(routes![list_todos])
        .config(config)
        .mount_mcp("/mcp")
        .build();

    let resp = client
        .post("/mcp")
        .header("origin", "https://app.example")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await;
    resp.assert_ok();
    assert_eq!(
        resp.header("access-control-allow-origin"),
        Some("https://app.example")
    );
}

#[tokio::test]
async fn structured_path_argument_is_rejected() {
    // Path params are advertised as `{"type":"string"}`; a `null`/object/array
    // must return `-32602` rather than replaying a literal `null`/JSON-text
    // path segment against a real resource.
    let client = TestApp::new()
        .routes(routes![get_todo])
        .mount_mcp("/mcp")
        .build();

    let out = rpc(
        &client,
        serde_json::json!({
            "jsonrpc":"2.0","id":9,"method":"tools/call",
            "params": {"name":"get_todo","arguments":{"id":{"nested":"object"}}}
        }),
    )
    .await;

    assert_eq!(out["error"]["code"], -32602);
}

#[tokio::test]
async fn secure_mcp_gates_the_whole_endpoint() {
    let store = Arc::new(InMemoryApiTokenStore::default());
    let token = issue_api_token(store.as_ref(), "agent:bot").await.unwrap();

    let client = TestApp::new()
        .routes(routes![list_todos])
        .mount_mcp("/mcp")
        .secure_mcp(RequireApiToken::new(store.clone()))
        .build();

    // Even the catalog requires the token now.
    client
        .post("/mcp")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .assert_status(401);

    let ok = client
        .post("/mcp")
        .header("authorization", &format!("Bearer {token}"))
        .json(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .send()
        .await;
    ok.assert_ok();
    let out = ok.json::<serde_json::Value>();
    assert!(out["result"]["tools"].is_array());
}
