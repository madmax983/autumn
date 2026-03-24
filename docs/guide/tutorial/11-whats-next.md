# Chapter 11: What's Next

**Goal:** By the end of this chapter, you will have a complete, working todo
application and a clear picture of where to go from here.

---

## Sections

### What You Built

Recap of the full application: project scaffolding, route macros, Postgres
with Diesel, Maud templates, Tailwind CSS styling, htmx interactivity, a
JSON API, structured error handling, and production configuration. All from
one framework dependency.

### Comparing to the Reference Implementation

Your code vs. `examples/todo-app/`. Any polish the example includes that the
tutorial omitted. Using the example as a continued reference.

### Ideas for Extending the App

- **Authentication** — add user accounts with session cookies
- **Categories** — associate todos with categories (a second table, foreign keys)
- **Search** — add a search bar with `ILIKE` queries
- **Pagination** — limit the list with `.limit()` and `.offset()`
- **Testing** — write integration tests with Autumn's test utilities

### Further Reading

- [API Reference](../../api/) — generated Rust docs for every public type
- [Getting Started Guide](../getting-started.md) — quick overview of all features
- [Example App](../../../examples/todo-app/) — the reference implementation
- [Autumn on crates.io](https://crates.io/crates/autumn) — versioned releases

### Community

Where to ask questions, report bugs, and contribute.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 10 — Configuration and Production Defaults](10-configuration.md) | Back to [Tutorial Index](index.md)
