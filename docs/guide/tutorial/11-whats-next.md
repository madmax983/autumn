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
tutorial omitted. Then branch out into the newer example apps:

- `examples/blog/` for hybrid rendering and a richer admin UI
- `examples/bookmarks/` for repository macros, generated CRUD APIs, and scheduled tasks
- `examples/wiki/` for mutation hooks and revision history

### Ideas for Extending the App

- **Authentication** \x97 add user accounts with session cookies
- **Actuator hardening** \x97 tune which operational endpoints stay visible in prod
- **Background work** \x97 add a `#[scheduled]` task for cleanup or polling
- **Categories** \x97 associate todos with categories (a second table, foreign keys)
- **Search** \x97 add a search bar with `ILIKE` queries
- **Pagination** \x97 limit the list with `.limit()` and `.offset()`
- **Testing** \x97 write integration tests with Autumn's test utilities

### Further Reading

- [API Reference](https://docs.rs/autumn-web) \x97 generated Rust docs for every public type
- [Getting Started Guide](../getting-started.md) \x97 quick overview of all features
- [Example App](../../../examples/todo-app/) \x97 the reference implementation
- [Autumn on crates.io](https://crates.io/crates/autumn-web) \x97 versioned releases

### Community

Where to ask questions, report bugs, and contribute.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 10 \x97 Configuration and Production Defaults](10-configuration.md) | Back to [Tutorial Index](index.md)
