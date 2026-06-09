---
name: routes
description: >
  Use when the user runs /autumn:routes, asks to list registered routes,
  inspect route handlers, check what endpoints exist, or audit an Autumn
  app's routing table.
argument-hint: "[--user-only] [--method GET|POST|...] [--filter <pattern>]"
allowed-tools:
  - Bash
  - Read
---

# autumn:routes

Run `autumn routes` to emit a machine-readable list of all registered routes
in the current Autumn project.

## Execution

Run from the project root (directory containing `autumn.toml`). Always use
JSON output. Include `--user-only` by default to hide framework internals,
but omit it when the user explicitly wants to see framework routes (actuator,
health probes, static assets, admin):

```bash
# Default — user routes only
autumn routes --format json --user-only

# When user wants all routes (framework + user)
autumn routes --format json
```

If the user passes `--method` or `--filter`, append them to whichever form is appropriate:

```bash
autumn routes --format json --user-only --method POST --filter /posts
```

Capture stdout, stderr, and exit code.

## Output handling

Parse the JSON array and present a clean table grouped by handler file or
resource:

```
Routes (N total):

  GET    /posts                    routes::posts::list
  GET    /posts/{id}               routes::posts::show
  POST   /posts           [auth]   routes::posts::create
  PATCH  /posts/{id}      [auth]   routes::posts::update
  DELETE /posts/{id}      [auth]   routes::posts::delete_post
```

Mark secured routes with `[auth]` and admin-only with `[admin]` if that
information is present in the JSON.

## Auto-mounted routes

Autumn automatically mounts these — they appear when `--user-only` is
omitted. They do not need to be shown unless the user asks:

- `GET /health`, `GET /actuator/*`, `GET /live`, `GET /ready`, `GET /startup`
- `GET /static/js/htmx.min.js`
- Admin routes when `autumn-admin-plugin` is installed

## When the project has not been built

If `autumn routes` fails because the project has not been compiled, tell the
user to run `cargo build` first, then retry.

## Comparing expected vs actual routes

When the user is debugging a 404, compare the route table with the requested
path. Common mismatches:
- Parameter syntax: routes use `{id}`, not `:id`
- Missing registration in `main.rs` `routes![...]`
- Method mismatch (POST handler hit with GET)
