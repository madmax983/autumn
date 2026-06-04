# Dev Error Overlay

When a handler panics, returns an `Err`, or propagates a `?`, Autumn's dev
profile renders a rich browser overlay instead of a plain 500 page. The overlay
gives you everything you need to diagnose the failure without leaving the tab.

## What it shows

| Section | Contents |
|---------|----------|
| **Error** | Status code, reason phrase, and the error message |
| **Stack trace** | Parsed Rust frames; workspace frames are expandable with ~10 lines of source context around the failing line |
| **Request** | Method, path, matched route pattern, request ID, query string, path params |
| **Headers** | Scrubbed request headers (sensitive keys replaced with `[FILTERED]`) |
| **Cookies** | Parsed session cookies, scrubbed by the same rules as headers |
| **SQL Queries** | Statements, bind counts, and durations — populated by autumn-harvest when present |

## Activation

The overlay is active automatically when:

1. The **dev profile** is in use (`AUTUMN_ENV=dev` or `--profile dev`, or when
   running `cargo run` without an explicit profile, which defaults to dev).
2. The request's `Accept` header prefers HTML (browser navigation).

API clients (`Accept: application/json`) always receive RFC 7807 Problem Details
regardless of profile.

The overlay is **never** shown in production. Two independent guards enforce
this: a runtime profile check and a `#[cfg(debug_assertions)]` guard on
backtrace capture. See [ADR 0006](../adr/0006-dev-error-overlay.md) for the
full reasoning.

## Triggering the overlay

### Handler `Err` return

```rust
#[get("/posts/{id}")]
async fn get_post(Path(id): Path<i32>) -> AutumnResult<Markup> {
    if id < 0 {
        return Err(AutumnError::bad_request_msg("id must be positive"));
    }
    // ...
}
```

Hit `/posts/-1` in a browser — the overlay pops up showing the bad-request
error, the request path, and (if autumn-harvest is wired up) any queries that
ran before the error.

### `?` propagation

```rust
#[get("/data")]
async fn load_data(db: Db) -> AutumnResult<Markup> {
    let rows = db.run(|conn| load_all(conn)).await?;  // ? becomes AutumnError
    // ...
}
```

Any `std::error::Error` propagated via `?` is wrapped as a 500. The overlay
shows the backtrace captured at the conversion point, with source context for
workspace frames.

### Intentional test route (e.g. `examples/reddit-clone`)

The `examples/reddit-clone` app ships a `/dev/trigger-error` route that panics
on purpose. Visit it in `cargo run -p reddit-clone` to see the overlay in action:

```
GET http://localhost:3000/dev/trigger-error
```

The route is registered only when the dev profile is active; it returns 404 in
production.

## Opt-out

If you prefer the plain 500 page without the badge overlay, set the profile to
production:

```toml
# autumn.toml
[app]
profile = "production"
```

Or at runtime:

```sh
AUTUMN_ENV=production cargo run
```

You can also provide a custom `ErrorPageRenderer` that renders whatever HTML you
prefer — the badge is only injected by the default pipeline when `is_dev` is
true.

## Sensitive parameter filtering

The overlay scrubs headers and cookies using the same `ParameterFilter` rules
configured in `autumn.toml`:

```toml
[log]
filter_parameters = ["pin", "ssn"]        # add to default list
unfilter_parameters = ["authorization"]   # remove from default list
```

Default scrubbed keys include `password`, `token`, `secret`, `authorization`,
`api_key`, `access_token`, `cookie`, and others. See
[Logging PII](logging-pii.md) for the full list.

## SQL queries (autumn-harvest)

When `autumn-harvest` is in the dependency graph, it pushes query records to the
overlay via the `DevBadgeContext.sql_queries` field. Each record shows the SQL
statement, bind parameter count, and duration in milliseconds. The overlay shows
a "SQL Queries (N)" section only when at least one query was recorded.

Without autumn-harvest the section is hidden; no configuration is needed.

## Source context

For stack frames inside the project workspace (relative file paths or absolute
paths inside the current directory), the overlay reads the source file from disk
and shows ±5 lines around the failing line, with the failing line highlighted
in red.

**Requirements:**
- The source files must be present on the same machine as the running process
  (true for local dev; not true for container builds where source is absent).
- The binary must be built in debug mode (`cargo run` or `cargo build`).

When source files are absent, the overlay still shows the full stack trace
(file, line, function name) without the inline code context.
