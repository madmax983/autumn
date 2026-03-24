# Chapter 8: JSON API

**Goal:** By the end of this chapter, you will have a JSON API at `/api/todos`
that supports listing and creating todos, testable with curl or any HTTP
client.

---

## Sections

### `Json<T>` as Request and Response

`Json<T>` serves double duty in Autumn: it extracts a JSON request body
(when used as a handler parameter) and serializes a response to JSON with the
correct `Content-Type` header (when used as a return type).

### Creating `src/routes/api.rs`

Adding a new route module for JSON endpoints alongside the HTML routes.
Registering the new routes in `main.rs`.

### GET `/api/todos` — List as JSON

Returning `AutumnResult<Json<Vec<Todo>>>`. The same query as the HTML list
handler, different response format.

### POST `/api/todos` — Create from JSON

Accepting `Json<NewTodo>` as input, inserting into the database, and
returning the created `Todo` with `returning()` and `get_result()`.

### Sharing Logic Between HTML and JSON Handlers

When to share query code between formats and when to keep them separate.
Practical patterns for code reuse.

### Testing with curl

Example curl commands for listing and creating todos through the JSON API.

### Checkpoint

Expected project state with both HTML and JSON routes.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 7 — Interactivity with htmx](07-htmx.md) | Next: [Chapter 9 — Error Handling](09-errors.md)
