# Chapter 9: Error Handling

**Goal:** By the end of this chapter, you will understand how `AutumnResult<T>`
and `AutumnError` turn Rust's `?` operator into automatic HTTP error
responses, and how to return the right status code for each error scenario.

---

## Sections

### The `?` Operator and Automatic 500s

Any `Error + Send + Sync` is converted into an `AutumnError` with status 500
via the blanket `From` impl. Use `?` freely in handlers — unexpected errors
become internal server errors with zero ceremony.

### `AutumnResult<T>` — the Standard Handler Return Type

`type AutumnResult<T> = Result<T, AutumnError>`. Why every handler that can
fail should return this type.

### Status Code Refinement

Using `.not_found()`, `.bad_request()`, and `.unprocessable()` to override
the default 500 status. Mapping Diesel's "not found" to HTTP 404.
`.with_status()` for any `StatusCode`.

### Validation Errors

Validating the todo title is not empty. Returning 422 Unprocessable Entity
with a meaningful error message.

### The JSON Error Response Format

`AutumnError` serializes to `{ "error": { "status": 404, "message": "..." } }`.
Consistent error shape for both HTML and JSON consumers.

### Checkpoint

Expected project state with proper error handling throughout.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 8 — JSON API](08-json-api.md) | Next: [Chapter 10 — Configuration and Production Defaults](10-configuration.md)
