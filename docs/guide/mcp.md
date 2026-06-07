# Exposing Your API as MCP Tools

Autumn already makes it fast to ship a typed JSON API. The `mcp` feature lets
an AI agent *use* that API by projecting your existing routes into a
[Model Context Protocol](https://modelcontextprotocol.io) (MCP) server — the
same way Autumn projects them into an OpenAPI document. You tag the endpoints
you want to expose and mount one server; Autumn derives the tool schemas,
speaks JSON-RPC over Streamable HTTP, and dispatches each tool call through
your **real, authenticated handler pipeline**.

No second app. No hand-written protocol, transport, or tool schemas. The tool
catalog is derived from the same `ApiDoc` metadata that drives `generate_spec`,
so it **cannot drift** from your handlers — change a handler's types and the
tool schema changes with it, with no extra edit.

---

## 1. Enable the feature

The `mcp` feature builds on the OpenAPI schema machinery, so it implies the
`openapi` feature.

```toml
# Cargo.toml
[dependencies]
autumn-web = { version = "0.5", features = ["mcp"] }
```

---

## 2. Tag endpoints and mount the server

Opt in **per endpoint** with `#[api_doc(mcp)]`, then mount the endpoint once:

```rust
use autumn_web::prelude::*;

#[derive(serde::Serialize, serde::Deserialize)]
struct Todo { id: u32, title: String }

#[derive(serde::Serialize, serde::Deserialize)]
struct NewTodo { title: String }

#[get("/api/todos")]
#[api_doc(mcp, summary = "List all todos")]
async fn list_todos() -> AutumnResult<Json<Vec<Todo>>> {
    Ok(Json(vec![Todo { id: 1, title: "first".into() }]))
}

#[post("/api/todos")]
#[api_doc(mcp, summary = "Create a todo")]
async fn create_todo(Json(body): Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    Ok(Json(Todo { id: 42, title: body.title }))
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![list_todos, create_todo])
        .mount_mcp("/mcp")
        .run()
        .await;
}
```

That's it. `POST /mcp` now speaks MCP and exposes `list_todos` and
`create_todo` as agent-callable tools.

**Opt-in is the default; nothing is exposed implicitly.** A route with no
`#[api_doc(mcp)]` tag never becomes a tool.

---

## 3. What the agent sees

`mount_mcp` serves a single Streamable-HTTP endpoint that handles the three
methods an MCP client needs:

| Method | Purpose |
|--------|---------|
| `initialize` | Handshake; returns `serverInfo` and `capabilities.tools`. |
| `tools/list` | The derived tool catalog. |
| `tools/call` | Invoke a tool by name; dispatched through the real pipeline. |

`ping` and JSON-RPC notifications (messages with no `id`, e.g.
`notifications/initialized`) are handled too — notifications get an empty
`202 Accepted`, per the spec.

A `tools/list` entry looks like this — `name`, `description`, `inputSchema`,
and `annotations` are all derived from the handler's `ApiDoc`:

```json
{
  "name": "create_todo",
  "description": "Create a todo",
  "inputSchema": {
    "type": "object",
    "properties": {
      "body": { "$ref": "#/$defs/NewTodo" }
    },
    "required": ["body"],
    "$defs": { "NewTodo": { "type": "object", "title": "NewTodo" } }
  },
  "annotations": { "title": "Create a todo", "readOnlyHint": false }
}
```

### How `inputSchema` is built

Autumn merges the handler's typed contract into one object schema:

- **Path parameters** (`/api/todos/{id}`) become required `string` properties
  named after each capture (`id`).
- A **`Query<T>` extractor** becomes a `query` object property.
- A **JSON request body** (`Json<T>`) becomes a required `body` property.
- Named component schemas are inlined under `$defs` so the schema is
  self-contained.

Because every piece comes from the same `SchemaEntry` data the OpenAPI
generator uses, **there is no second schema to maintain** and no way for the
tool catalog to drift from the handler.

### Safety annotations

The HTTP method maps to MCP safety hints so agents and UIs can reason about
side effects:

| Verb | `readOnlyHint` | `destructiveHint` |
|------|:--------------:|:-----------------:|
| `GET` | `true` | — |
| `POST` / `PUT` / `PATCH` | `false` | — |
| `DELETE` | `false` | `true` |

---

## 4. Calling a tool

`tools/call` takes a tool `name` and an `arguments` object whose shape mirrors
the `inputSchema`: path parameters at the top level, query fields under
`query`, and the JSON body under `body`.

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "create_todo",
    "arguments": { "body": { "title": "buy milk" } }
  }
}
```

The handler's JSON response comes back as the tool result's text content:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [{ "type": "text", "text": "{\"id\":42,\"title\":\"buy milk\"}" }],
    "isError": false
  }
}
```

A non-2xx handler response is returned as `isError: true` with the status and
body, rather than a transport-level failure — so an agent can read and recover
from a validation error the same way a human-written client would.

You can drive it from the command line:

```bash
curl -s http://127.0.0.1:3000/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

---

## 5. Authentication: reuse your bearer tokens

`tools/call` runs through the **real handler pipeline** — the same in-process
path Autumn's [test client](testing.md) uses. That means `#[secured]`,
authorization, tenancy, rate limits, and validation all apply identically to
an agent call and an ordinary HTTP call. There is no separate auth subsystem.

Agents authenticate exactly like any other API client: with a bearer token
verified by [`RequireApiToken`](../../autumn/src/auth.rs). The `Authorization`
header an agent sends to `/mcp` is **forwarded** into the dispatched request,
so the call runs as that verified principal.

To put a tool behind token auth, register the route inside a `scoped` group
carrying the `RequireApiToken` layer. The scope keeps the route in the
registry (so MCP can derive the tool) *and* applies the layer (so every call —
agent or HTTP — is checked):

```rust
use std::sync::Arc;
use autumn_web::auth::{InMemoryApiTokenStore, RequireApiToken};

let store = Arc::new(InMemoryApiTokenStore::default());

autumn_web::app()
    .scoped(
        "/api",
        RequireApiToken::new(store.clone()),
        routes![list_todos, create_todo], // handlers declared at "/todos"
    )
    .mount_mcp("/mcp")
    .run()
    .await;
```

A `tools/call` with no token is rejected by `RequireApiToken` and surfaces as
`isError: true`; the same call with a valid `Authorization: Bearer <token>`
header on the `/mcp` request succeeds.

---

## 6. The whole-API hatch

For internal tools or trusted agents you can expose every eligible **read**
endpoint at once, without tagging each one, via `expose_all_as_mcp()`:

```rust
autumn_web::app()
    .routes(routes![/* ... */])
    .expose_all_as_mcp()   // mounts at /mcp; chain mount_mcp("/path") to change it
    .run()
    .await;
```

This is an explicit, separate opt-in — never the default. It is deliberately
conservative:

- **`GET` endpoints are auto-included**, but **mutating verbs
  (`POST`/`PUT`/`PATCH`/`DELETE`) still require an explicit `#[api_doc(mcp)]`
  opt-in.** A write is never exposed implicitly, even under the hatch.
- **Per-endpoint exclusions are always honored.** Mark a route
  `#[api_doc(mcp = false)]` to keep it out, even under `expose_all_as_mcp()`.

---

## 7. Eligibility: JSON in, JSON out

Only JSON endpoints are eligible. Autumn detects this structurally: a route is
eligible when its handler has a JSON **response schema** (it returns
`Json<T>`). HTML/Maud routes have no response schema and are **auto-excluded**.

If you tag an HTML route with `#[api_doc(mcp)]`, it is skipped with a
build-time log note rather than a runtime surprise:

```text
WARN skipping MCP exposure: endpoint has no JSON response schema
     (HTML/Maud routes are not eligible as MCP tools)
     operation_id="dashboard" method="GET" path="/dashboard"
```

---

## 8. End-to-end example

`examples/todo-app` ships an `/mcp` endpoint. Its bearer-token JSON API
(`list_json`, `create_json`) is mounted in a `scoped("/api", RequireApiToken,
…)` group and tagged `#[api_doc(mcp)]`, exposing one read tool and one
explicitly-opted-in write tool behind the same token auth a mobile client uses:

```rust
.scoped(
    "/api",
    RequireApiToken::new(Arc::new(deferred.clone())),
    routes![routes::api::list_json, routes::api::create_json],
)
.mount_mcp("/mcp")
```

Run it, issue a token via `POST /api/tokens`, then `tools/list` and
`tools/call` against `/mcp` with that token in the `Authorization` header.

---

## 9. How a tool call is dispatched (and why it can't loop)

When a `tools/call` arrives, the MCP handler reconstructs an ordinary HTTP
request — filling the path template, building the query string, and attaching
the JSON body — forwards the `Authorization` header, and replays it through a
clone of the fully-assembled application router. Because it traverses the same
routes, layers, and middleware an external request would, security and
validation are *shared*, not re-implemented.

The dispatch target is a **snapshot of the router taken before the `/mcp`
route is merged in**. So the routing graph is acyclic by construction:

```text
agent → POST /mcp → serve_mcp → dispatch (a router that has no /mcp route)
                                   → your handler → JSON → tool result
```

A tool call resolves to a normal handler in one hop and returns; it can never
re-enter the MCP endpoint. (Tool paths are derived only from your route
registry and are never `/mcp` to begin with — the pre-merge snapshot upgrades
that from convention to a structural guarantee.)

> **Caveat — mount path collisions.** Autumn does not yet pre-check that your
> chosen `mount_mcp` path is free. If a real handler is already mounted at the
> same path, `axum` panics at startup on the duplicate route (loud and early,
> not a silent failure). Pick a path you don't otherwise serve — `/mcp` is the
> convention.

---

## 10. Scope and roadmap

This slice is intentionally **tools-only and buffered**. The following are
**out of scope for v1** and tracked as follow-ups:

- **Streaming / partial tool results** (SSE-style progressive output) — rides
  the existing `sse.rs` transport. Tracked in
  [#1118](https://github.com/madmax983/autumn/issues/1118). Base tool exposure
  stays buffered happy-path; streaming layers on top.
- **Daemon lifetime** for long-running, session-surviving work —
  [#1119](https://github.com/madmax983/autumn/issues/1119).
- **Durable workflow tools** — exposing Harvest `#[workflow]`s as
  start/status/signal MCP tools on top of this layer
  ([autumn-harvest#597](https://github.com/madmax983/autumn-harvest)).
- **MCP resources, prompts, and sampling** — this slice is tools-only.
- **stdio transport** — agents target deployed apps, so HTTP only for v1.
- **Non-JSON endpoints** (file upload/download, HTML).
- **LLM-assisted tool descriptions** — descriptions come from `#[api_doc]`;
  garbage-in/garbage-out is the author's call.

For the typed JSON-Schema derivation this builds on, see the OpenAPI support
in [`openapi.rs`](../../autumn/src/openapi.rs); for the in-process dispatch
path, see the [Testing guide](testing.md).
