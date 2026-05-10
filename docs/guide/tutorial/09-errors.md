# Chapter 9: Error Handling

**Goal:** By the end of this chapter, you will understand how `AutumnResult<T>`
and `AutumnError` turn Rust's `?` operator into automatic HTTP error
responses, and how to return the right status code for each error scenario.

---

## The `?` Operator and Automatic 500s

Any error type implementing `std::error::Error + Send + Sync + 'static` can be
converted into `AutumnError`.

That means handler code can stay idiomatic:

```rust,no_run
use autumn_web::prelude::*;

#[get("/read-config")]
async fn read_config() -> AutumnResult<String> {
    let text = std::fs::read_to_string("config.json")?;
    Ok(text)
}
```

If the file read fails, Autumn converts the error into a `500 Internal Server
Error` response automatically.

---

## `AutumnResult<T>` — the Standard Handler Return Type

Use this return type for handlers that can fail:

```rust
type AutumnResult<T> = Result<T, AutumnError>;
```

This keeps signatures consistent and makes error intent obvious.

---

## Status Code Refinement

Using `.not_found()`, `.bad_request()`, and `.unprocessable()` to override
the default 500 status. Mapping Diesel's "not found" to HTTP 404.
`.with_status()` for any `StatusCode`.

```rust,no_run
use autumn_web::prelude::*;

#[get("/posts/{id}")]
async fn show_post(id: i32) -> AutumnResult<String> {
    if id <= 0 {
        return Err(AutumnError::new("invalid id").bad_request());
    }

    // Pretend we looked up a DB row.
    Err(AutumnError::new("post not found").not_found())
}
```

---

## Validation Errors

Validating the todo title is not empty. Returning 422 Unprocessable Entity
with a meaningful error message.

```rust,no_run
use autumn_web::prelude::*;

#[post("/todos")]
async fn create_todo(
    Json(payload): Json<serde_json::Value>,
) -> AutumnResult<Json<serde_json::Value>> {
    let title = payload
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if title.is_empty() {
        return Err(AutumnError::new("title is required").unprocessable());
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
```

---

## HTML Error Page Overrides (404, 500, 422)

Autumn ships with styled default HTML pages for browser requests.

To customize them globally, install your own
`autumn_web::error_pages::ErrorPageRenderer` on `AppBuilder`:

```rust,no_run
use autumn_web::error_pages::{ErrorContext, ErrorPageRenderer};
use autumn_web::prelude::*;
use maud::{html, Markup};

struct BrandedErrors;

impl ErrorPageRenderer for BrandedErrors {
    fn render_404(&self, ctx: &ErrorContext) -> Markup {
        html! { h1 { "Custom 404 for " (ctx.path) } }
    }

    fn render_500(&self, ctx: &ErrorContext) -> Markup {
        html! {
            h1 { "Custom 500" }
            @if let Some(id) = &ctx.request_id {
                p { "Request ID: " (id) }
            }
        }
    }

    fn render_422(&self, ctx: &ErrorContext) -> Markup {
        html! {
            h1 { "Custom 422" }
            p { (ctx.message) }
            @if let Some(details) = &ctx.details {
                ul {
                    @for (field, errors) in details {
                        li {
                            b { (field) ": " }
                            (errors.join(", "))
                        }
                    }
                }
            }
        }
    }

    fn render_error(&self, ctx: &ErrorContext) -> Markup {
        html! { h1 { "Fallback " (ctx.status.as_u16()) } }
    }
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .error_pages(BrandedErrors)
        .run()
        .await;
}
```

`ErrorContext` gives your template everything needed for branded pages:

- `status` — HTTP status code
- `path` — request path
- `request_id` — request identifier (when available)
- `details` — validation map for 422 errors
- `message` and `is_dev` — additional error context

Important: HTML overrides apply only when the request prefers `text/html`.
`application/json` requests continue to receive structured JSON errors.

---

## Problem Details JSON Error Contract

Framework-generated JSON/API errors use Problem Details semantics and the
`application/problem+json` media type. The stable contract is:

```json
{
  "type": "https://autumn.dev/problems/not-found",
  "title": "Not Found",
  "status": 404,
  "detail": "No route matches /missing",
  "instance": "/missing",
  "code": "autumn.not_found",
  "request_id": "018f4f30-6b7c-4b4c-8dc0-70a2c8d7f97d",
  "errors": []
}
```

Clients may depend on `type`, `title`, `status`, `detail`, `instance`,
`code`, `request_id`, and `errors`. Validation failures put field-level
details in `errors`:

```json
{
  "type": "https://autumn.dev/problems/validation-failed",
  "title": "Validation Failed",
  "status": 422,
  "detail": "Validation failed",
  "instance": "/todos",
  "code": "autumn.validation_failed",
  "request_id": "018f4f30-6b7c-4b4c-8dc0-70a2c8d7f97d",
  "errors": [
    { "field": "title", "messages": ["validation failed: length"] }
  ]
}
```

Production-profile 5xx responses use a client-safe `detail` such as
`"Internal server error"` and rely on `request_id` for support correlation.
Development profile includes the original diagnostic detail in JSON, while
logs retain the operator-facing cause in both profiles.

| Scenario | Status | `code` |
|----------|--------|--------|
| Malformed path/query/body | 400 | `autumn.bad_request` |
| Authentication required | 401 | `autumn.unauthorized` |
| Authorization denied | 403 or 404 | `autumn.forbidden` or `autumn.not_found` |
| Route not found | 404 | `autumn.not_found` |
| Repository conflict | 409 | `autumn.conflict` |
| Validation failure | 422 | `autumn.validation_failed` |
| Internal error | 500 | `autumn.internal_server_error` |
| Missing service dependency | 503 | `autumn.service_unavailable` |

Content negotiation stays explicit: requests preferring `text/html` continue
through the HTML error-page renderer, while JSON/API requests receive Problem
Details. htmx/form helpers keep their existing form-oriented flow unless they
ask for JSON.

Migration note: pre-1.0 clients that parsed
`{ "error": { "status": ..., "message": ... } }` should switch to `status`,
`detail`, and `code` at the top level. OpenAPI generation publishes a shared
`ProblemDetails` schema so generated clients can use one error type.

---

### Checkpoint

Expected project state with proper error handling throughout.

---

Previous: [Chapter 8 — JSON API](08-json-api.md) | Next: [Chapter 10 — Configuration and Production Defaults](10-configuration.md)
