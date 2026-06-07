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
