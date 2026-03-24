# Chapter 2: Routes and Handlers

**Goal:** By the end of this chapter, you will have a root route that redirects
to `/todos`, a GET `/todos` handler that returns a placeholder page, and a
clear understanding of how Autumn's route macros work.

---

## Sections

### Replacing the Scaffold Routes

Remove the generated hello-world handlers and define routes for the todo app.

### The `#[get]` and `#[post]` Macros

How route macros translate your function signatures into Axum handlers. Method
macros available: `#[get]`, `#[post]`, `#[put]`, `#[delete]`.

### Path Parameters with `{name}` Syntax

Using `Path<T>` to extract typed values from URL segments. The todo app will
use `Path<i32>` for todo IDs.

### The `routes![]` Collection Macro

How `routes![]` collects handlers into a `Vec<Route>` and why you can call
`.routes()` multiple times on the app builder to compose route groups.

### Organizing Routes into Modules

Moving handlers into `src/routes/todos.rs` and `src/routes/mod.rs`. Module
structure conventions for larger applications.

### Return Types

What a handler can return: `&str`, `String`, `Markup`, `Json<T>`,
`AutumnResult<T>`. How Axum's `IntoResponse` trait works under the hood.

### Checkpoint

Expected project state with the new route structure.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 1 — Project Setup](01-project-setup.md) | Next: [Chapter 3 — Database Setup](03-database.md)
