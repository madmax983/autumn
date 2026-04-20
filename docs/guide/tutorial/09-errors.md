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

## The JSON Error Response Format

`AutumnError` serializes to `{ "error": { "status": 404, "message": "..." } }`.
Consistent error shape for both HTML and JSON consumers.

---

### Checkpoint

Expected project state with proper error handling throughout.

---

Previous: [Chapter 8 — JSON API](08-json-api.md) | Next: [Chapter 10 — Configuration and Production Defaults](10-configuration.md)
