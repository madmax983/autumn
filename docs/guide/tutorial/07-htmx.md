# Chapter 7: Interactivity with htmx

**Goal:** By the end of this chapter, you will be able to toggle a todo's
completion status and delete a todo without a full page reload, using htmx
attributes and server-rendered HTML fragments.

---

## Sections

### What htmx Does

htmx sends AJAX requests triggered by HTML attributes and swaps the response
(an HTML fragment) into the DOM. No JavaScript to write. Autumn bundles htmx
and serves it at `/static/js/htmx.min.js`.

### Why Autumn Chose htmx

Server-rendered HTML with sprinkles of interactivity. No client-side
framework, no build step for JS, no JSON serialization/deserialization for
UI interactions. The server stays the source of truth.

### Toggle: `hx-post` and Partial Responses

Adding `hx-post="/todos/{id}/toggle"` to the checkbox button. The server
handler toggles the `completed` flag, re-renders the single `<li>` item, and
returns it. htmx swaps the old `<li>` with the new one via `hx-target` and
`hx-swap="outerHTML"`.

### Delete: `hx-delete` and Empty Responses

Adding `hx-delete="/todos/{id}"` to the delete button. The server deletes
the record and returns an empty string. htmx replaces the element with
nothing, removing it from the page.

### Understanding `hx-target` and `hx-swap`

How htmx knows which element to update and how to update it. The `outerHTML`
swap strategy vs. `innerHTML`.

### The Fragment Pattern

Returning partial HTML (a single `<li>`, not a full page) from htmx
endpoints. Why this works and how it differs from SPA patterns.

### Checkpoint

Expected project state with interactive toggle and delete.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 6 — Styling with Tailwind CSS](06-tailwind.md) | Next: [Chapter 8 — JSON API](08-json-api.md)
