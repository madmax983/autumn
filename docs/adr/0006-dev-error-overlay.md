# ADR 0006: Dev-Mode Error Overlay

- Status: Accepted
- Date: 2026-06-03
- Deciders: Autumn maintainers
- Tags: dx, dev-mode, error-pages, middleware, security

## Context

When a handler panics, returns an `Err`, or hits a template error during `autumn
dev`, the developer sees a terminal stack trace and a generic 500 in the browser.
The developer must then context-switch: copy the file path out of the trace, open
it in an editor, scroll to the line, and mentally reconstruct the request that
triggered it.

Rails (`better_errors`), Phoenix (`Plug.Debugger`), Laravel (`Ignition`), and
Django (technical 500) all collapse this loop into one browser page — source
snippet at the failing line, request params, headers, session, and (in some
cases) the SQL queries that ran on that request.

Autumn's DX bet — time-to-deployed-app — is undermined every minute spent
re-routing around a 500. This ADR records the decisions that shape the dev error
overlay and its **hard production-safety invariants**.

## Decision

Autumn ships a first-party dev-mode error overlay rendered as a floating badge
injected into HTML error responses. The overlay is:

1. **Never activated in production** — gated by two independent conditions
   (see below).
2. **Zero-dependency** — uses Maud (already required) and inline CSS only; no
   new template engine or frontend build step.
3. **Non-breaking** — the overlay is injected *after* the existing HTML error
   page, not replacing it. JSON/API requests that prefer `application/json`
   continue to receive RFC 7807 Problem Details unchanged.
4. **Plugin-extensible** — a third-party plugin can replace the renderer (theme
   it, add hints) without forking the framework; the "framework owns the boundary,
   plugins fill it" pattern from ADR 0005 applies here too.

## Production-Safety Invariant

**The overlay must never be served in a production environment.** Two independent
conditions must both be true for the overlay to activate:

| Condition | Mechanism | Layer |
|-----------|-----------|-------|
| Runtime dev profile | `config.profile == "dev" \| "development"` | `ErrorPageFilter.is_dev` flag set in `router.rs` |
| Debug build | `#[cfg(debug_assertions)]` | Backtrace capture in `AutumnError` constructors |

- **Profile check** (`is_dev`): determined at app startup from `AUTUMN_ENV`,
  `AUTUMN_PROFILE`, `--profile` CLI flag, or auto-detection from Rust build
  mode. The flag is threaded into `ErrorPageFilter` and never changes after the
  router is built. A production binary deployed with `AUTUMN_ENV=prod` (or with
  no override, defaulting to the release-mode auto-detection) will have
  `is_dev = false` and the overlay injection code is never reached.
- **Debug-assertions guard**: `std::backtrace::Backtrace::force_capture()` is
  called inside `#[cfg(debug_assertions)]` blocks only. Release binaries never
  capture or store backtrace strings. Even if `is_dev` were somehow set to `true`
  in a production binary (e.g., misconfigured environment), the overlay would
  render without a stack trace and without source context — no sensitive internal
  paths or frame symbols would leak.

Together these two conditions mean the meaningful overlay content is only present
when both conditions are met: dev profile AND debug build. Release binaries in
any profile produce no more than the existing styled HTML error page.

## Middleware Ordering and Activation

The overlay is **not a separate route**. It is injected by `ErrorPageFilter`, an
`ExceptionFilter` already present in the middleware chain. In dev profile:

```
ExceptionFilterLayer
  └── ProblemDetailsFilter (normalises to RFC 7807)
  └── ErrorPageFilter (renders HTML; injects badge when is_dev=true)
```

`ErrorPageFilter` only injects the badge when:
1. `is_dev` is `true` (profile check above).
2. The request's `Accept` header prefers HTML (`WantsHtml(true)`).
3. The response carries an `AutumnErrorInfo` extension (i.e., an `AutumnError`
   propagated through the handler).

Route pattern capture is provided by a `route_layer` middleware
(`capture_matched_path_middleware`) mounted inside the axum router. This layer
runs after route matching (so `MatchedPath` is available) and stores the matched
pattern in response extensions. The outer `ErrorPageContextService` reads it when
building `ErrorPageRequestContext`. The route_layer is only mounted when
`is_dev_profile` is true.

## Opt-Out

Applications that prefer the bare 500 page without the badge can disable it by
setting the profile to production or by passing a custom `ErrorPageRenderer` that
does not include the badge. One-line opt-out in `autumn.toml`:

```toml
[app]
profile = "production"
```

Or at runtime:

```sh
AUTUMN_ENV=production cargo run
```

## Consequences

### Positive

- Developer diagnoses failures (file, line, params, SQL) without leaving the
  browser or grepping a terminal.
- Production safety is defended at two independent layers; a single misconfiguration
  cannot expose the overlay.
- No new dependencies; the overlay is self-contained Maud + inline CSS.
- Autumn-harvest (when present) can populate the SQL queries section by pushing
  to the `sql_queries` field of `DevBadgeContext`; the overlay degrades gracefully
  when harvest is absent.

### Negative

- The overlay is injected just before `</body>` in existing HTML error pages,
  adding ~6 KB of inline CSS per error response in dev. Acceptable in dev; never
  reached in prod.
- Source-file reading at overlay-render time requires the source files to exist
  on the same filesystem as the running binary (true for local dev, not for
  container deployments where source is absent). When files are missing, the
  overlay degrades gracefully (shows the stack trace without source context).

### Risks

- A developer who manually sets `AUTUMN_ENV=dev` on a production server would
  activate the profile check. The debug-assertions guard still prevents backtrace
  and source data from appearing in a release binary, but the badge div and
  collapsible overlay HTML would be injected into HTML error pages. The ADR
  documents this as a misconfiguration risk, not a security boundary break. Ops
  runbooks must not set `AUTUMN_ENV=dev` in production.

## Alternatives Considered

### 1. Separate route endpoint (e.g. `/__autumn/error-detail`)

Rejected. A separate route requires stashing error state (backtrace, request
context) in a shared store between the handler and the debug endpoint. This adds
state-management complexity and introduces a race between error occurrence and
inspection. Phoenix Plug.Debugger inlines the page; we follow the same pattern.

### 2. New template engine (Tera, Askama)

Rejected. Autumn already depends on Maud and the overlay is self-contained HTML.
Adding a template engine for one feature would increase compile times and
dependency surface.

### 3. Opt-in configuration flag

Rejected. An opt-in flag creates friction for every new contributor. Dev-mode
activation is automatic when the profile is "dev". Users who prefer the bare 500
can opt out with a one-liner (see above) rather than opt in.

## Follow-On Work

- #801 (autumn-harvest integration): populate `sql_queries` by pushing to the
  `DevBadgeContext` from the harvest query instrumentation.
- Consider a `DevOverlayPlugin` trait so third-party crates can theme or augment
  the overlay (solution hints, Ignition-style) without forking.
- Once the overlay is stable, add a CI scenario that scripts the "handler error
  to identified failing line" flow and asserts it completes in under 5 seconds
  (target from the issue's success metric).
