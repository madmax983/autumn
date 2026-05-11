# autumn-admin-plugin

`autumn-admin-plugin` adds a server-rendered admin panel to an `autumn-web`
 application. Register one or more models, mount the plugin, and it serves a
 CRUD UI with list/detail/edit screens, search, filtering, bulk actions, CSRF
 integration, and HTMX-driven interactions with no frontend build step.

## Features

- Mounts an admin UI under `/admin` by default
- Generates list, create, detail, edit, delete, and bulk-action flows
- Includes a built-in jobs dashboard for queued, running, completed, and failed work
- Uses Maud + HTMX and works under Autumn's default `Content-Security-Policy`
- Reads and writes model data through a small `AdminModel` trait
- Requires an authenticated session with the `"admin"` role by default

## Installation

Add the plugin alongside `autumn-web`:

```toml
[dependencies]
autumn-web = { version = "0.4", features = ["db", "flash", "htmx", "maud"] }
autumn-admin-plugin = "0.4"
```

`autumn-admin-plugin` expects a configured Autumn database pool for registered
admin models because model operations receive the app's Postgres pool. The
built-in jobs dashboard can render without a database pool when no model route
is accessed.

## Quick Start

```rust,ignore
use autumn_admin_plugin::{prelude::*, AdminPlugin};

struct ProjectAdmin;
// Implement `AdminModel` for your type.
// Supply field metadata plus `list`, `get`, `create`, `update`, and `delete`.

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .plugin(AdminPlugin::new().register(ProjectAdmin))
        .run()
        .await;
}
```

## What the Plugin Mounts

When mounted at the default `/admin` prefix, the plugin serves:

- `GET /admin/` — dashboard
- `GET /admin/jobs` — background jobs dashboard
- `GET /admin/jobs/counters` — HTMX counter fragment, refreshed at most every 2s
- `POST /admin/jobs/{id}/retry` — retry a failed job
- `POST /admin/jobs/{id}/discard` — discard a failed job
- `POST /admin/jobs/{id}/cancel` — cancel an enqueued job
- `GET /admin/{slug}` — paginated list view
- `POST /admin/{slug}` — create record
- `GET /admin/{slug}/new` — new-record form
- `GET /admin/{slug}/{id}` — detail view
- `POST /admin/{slug}/{id}` — update record
- `DELETE /admin/{slug}/{id}` — delete record
- `GET /admin/{slug}/{id}/edit` — edit form
- `POST /admin/{slug}/actions` — bulk actions

The plugin also serves a hashed same-origin JavaScript asset under
`/admin/static/admin.<hash>.js` so long-lived caching stays safe across deploys.

## `AdminModel` Contract

Each registered model supplies:

- A URL slug and singular/plural display names
- A field schema via `Vec<AdminField>`
- `list`, `get`, `create`, `update`, and `delete` operations

`AdminField` covers the common form/display shapes: `Text`, `TextArea`,
`Integer`, `Float`, `Boolean`, `Date`, `DateTime`, `Select`, `Hidden`,
`Password`, and `Json`.

Optional hooks let you customize:

- `actions()` for extra bulk actions beyond the built-in delete action
- `execute_action()` to implement those custom actions
- `record_display()` for breadcrumbs and page titles
- `per_page()` and `count()` for pagination behavior

All values flow through `serde_json::Value` so the plugin stays object-safe and
does not need to know your application's concrete model types.

## Configuration

`AdminPlugin::new()` defaults to:

- Prefix: `/admin`
- Required role: `"admin"`
- Session auth key: `"user_id"`
- Actuator prefix: `/actuator`

You can override those defaults with:

- `prefix(...)`
- `require_role(...)`
- `auth_session_key(...)`
- `actuator_prefix(...)`

Example:

```rust,ignore
let plugin = AdminPlugin::new()
    .prefix("/backoffice")
    .actuator_prefix("/ops")
    .auth_session_key("uid")
    .require_role(Some("staff".to_owned()));
```

## Security Notes

- The plugin assumes Autumn session/auth middleware is already configured
- Role checks run before any admin handler by default
- CSRF tokens are rendered automatically when Autumn's `CsrfLayer` is enabled
- No inline JavaScript is used; the UI is compatible with Autumn's default CSP
- `Password` fields are treated as write-only and never rendered back to users

## Status

This crate is intended as the first-party admin plugin for `autumn-web`. The
API is pragmatic and functional, but still young enough that you should expect
incremental improvements around model ergonomics, docs, and batteries-included
examples.
