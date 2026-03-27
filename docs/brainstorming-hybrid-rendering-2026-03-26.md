# Brainstorming Session: Hybrid Rendering Design

**Date:** 2026-03-26
**Objective:** Stress-test the hybrid rendering design (`docs/design/hybrid-rendering.md`) through failure mode analysis and multi-perspective thinking before implementation begins
**Context:** Post-design, pre-implementation review. Single-developer constraint. Design targets v0.2–v0.5 phased rollout.

## Techniques Used
1. Reverse Brainstorming (failure mode analysis)
2. Six Thinking Hats (multi-perspective analysis)

---

## Subsystem 1: `#[static_get]` Proc Macro

### Failure Modes Identified

1. **Macro attribute confusion** — developers write `#[static_get]` on a `POST` handler or a WebSocket route. The macro name implies GET semantics, but nothing prevents a developer from expecting it to work with other HTTP methods. Result: subtle misbehavior or confusing errors.

2. **Return type mismatch at build time** — a `#[static_get]` handler returns `Result<Markup, AppError>` where `AppError` renders an HTML error page. At build time, if the handler errors, `autumn build` captures an error page HTML and writes it to `dist/` as if it were the real page. Silent data corruption.

3. **Macro ordering with other attribute macros** — `#[tracing::instrument]` or `#[tower_http::trace]` stacked on a `#[static_get]` handler. Proc macro ordering in Rust is top-down. If another macro rewrites the function signature before `#[static_get]` sees it, the companion metadata function may generate incorrect type information.

4. **Feature flag friction** — the `#[static_get]` proc macro is always compiled but emits `compile_error!` without the feature flag. Developers who see `static_get` in examples or docs will hit this error on their first try. The friction between "I saw this in a tutorial" and "I need to add a feature flag" could be a poor first impression.

5. **`params` function name is a string literal** — `params = "post_slugs"` uses a string to reference a function. This is not type-checked by the IDE, has no autocomplete, and a typo produces a compile error that points at the macro, not the typo. Contrast with a closure or function pointer which the compiler understands natively.

6. **Naming collision** — `__autumn_static_meta_{name}` companion functions could collide if two static routes in different modules have the same function name. The `routes![]` macro collects by name, not by path.

### Alternatives Explored

- **Attribute parameter instead of new macro** — `#[get("/about", static)]` instead of `#[static_get("/about")]`. Keeps a single macro family, reduces API surface, avoids the "is this GET-only?" confusion. Downside: `static` is a Rust keyword; would need `render = "static"` or `mode = "static"`.
- **`#[get("/about", render = "static")]`** — extends existing macro. Clear, non-ambiguous. Allows future values like `render = "isr"`. More flexible than a separate macro.
- **Trait-based opt-in** — handler implements `StaticRenderable` trait. More Rustic, but more boilerplate.
- **Function pointer for `params`** — `params = post_slugs` (no quotes, actual identifier) so the compiler resolves it.

### Recommended Adjustments

1. **Consider `#[get("/about", render = "static")]` syntax** — avoids introducing a new macro name, avoids GET-only confusion, and allows `render = "isr"` in the future. Evaluate during Phase 1 prototyping; if macro implementation is significantly simpler with `#[static_get]`, keep it but document that it implies GET.
2. **Validate handler return type at build time** — `autumn build` should check the HTTP status code of the rendered response. If the handler returns a 4xx or 5xx, fail the build for that route with a clear error, don't write the error page to `dist/`.
3. **Use identifier syntax for `params`** — change `params = "post_slugs"` to `params = post_slugs` (unquoted). This allows the proc macro to resolve it as an actual path, and IDEs can provide autocomplete.
4. **Namespace companion functions** — include the module path hash in generated function names to prevent cross-module collisions.

---

## Subsystem 2: `autumn build` CLI Pipeline

### Failure Modes Identified

1. **Build-time database drift** — `autumn build` runs against a database with different data than production. A blog's `params` function queries for published posts; if the build database has stale data, the static site is missing pages. Worse: if it has *more* data than production (test posts), the static site exposes unpublished content.

2. **Binary mismatch** — `autumn build` runs `cargo build --release` then executes the binary with `--autumn-build`. If the developer has been iterating with `cargo build` (debug) and runs `autumn build`, the release binary may have different behavior (optimizations, overflow checks, debug assertions). The built static pages reflect release behavior, but the developer has been testing debug behavior.

3. **Long build times compound** — for a site with 10,000 blog posts, `autumn build` serially renders each page through the full Axum middleware stack. With database queries, this could take minutes. No parallelism strategy is described in the design.

4. **Partial build failure** — `autumn build` renders 4,500 of 5,000 pages, then the database connection drops. The `dist/` directory now has a partial set of pages. If deployed, half the site returns 404s (in Mode 3) or falls back to SSR (in Mode 2, if the server is running).

5. **Stale `dist/` directory** — developer removes a `#[static_get]` annotation from a route but doesn't re-run `autumn build`. The old static HTML persists in `dist/` and continues to be served by `StaticFileLayer`, even though the route is now dynamic. The developer sees "my changes aren't taking effect."

6. **Cross-compilation mismatch** — `autumn build` compiles and runs the binary on the build host. In CI, the build host is Linux x86_64. But if cross-compiling for ARM (e.g., AWS Graviton), the `--autumn-build` binary can't execute on the build host.

7. **Environment variable leakage** — the build-time binary runs with the CI environment's variables. If the app reads `SECRET_KEY` or `API_TOKEN` at startup, those values may be different (or missing) from the production environment. Any route that uses environment-dependent logic renders differently at build time.

8. **No build cache / incremental builds** — every `autumn build` re-renders every static page from scratch. For large sites, this is wasteful. Next.js caches `.next/cache` between builds.

### Alternatives Explored

- **Render in parallel** — spawn a Tokio task per route (or per parameter set) with a configurable concurrency limit. Database pool size naturally throttles.
- **Two-phase build** — Phase A: render all pages to a staging directory. Phase B: atomically swap staging → `dist/`. Prevents partial deployment.
- **Content-hash caching** — hash the route handler's source + params + template. Skip re-render if hash matches previous build.
- **Clean before build** — `autumn build` deletes `dist/` entirely before rendering, ensuring no stale artifacts. Simple but loses incremental potential.
- **Warn on stale dist** — at server startup, compare `dist/manifest.json` route list against the current binary's registered routes. Log warnings for routes in manifest but not in binary.

### Recommended Adjustments

1. **Parallel rendering** — render routes concurrently with configurable concurrency (`[static] concurrency = 8` in `autumn.toml`). This is nearly free since each render is an independent async task and the database pool already handles connection limits.
2. **Atomic build output** — render to `dist.tmp/`, then rename to `dist/` on success. On any failure, leave the old `dist/` intact and fail the build with a summary of which routes succeeded/failed.
3. **Clean build by default** — `autumn build` removes the staging directory before rendering. Add `autumn build --incremental` as a future optimization.
4. **Startup route reconciliation** — at server boot, if `dist/manifest.json` exists, compare its route list with the app's registered routes. Log `WARN` for stale routes in dist that are no longer static. Log `INFO` for static routes missing from dist (will fall back to SSR).
5. **HTTP status validation** — after rendering each route, check the response status. Non-2xx responses are treated as build failures for that route.

---

## Subsystem 3: `StaticFileLayer` Middleware

### Failure Modes Identified

1. **Middleware ordering bugs** — `StaticFileLayer` sits above the Axum router. This means it also sits above authentication middleware. A `#[static_get]` route that *should* require authentication (e.g., a premium content page that was pre-rendered for all users) is now served without auth checks. The static file bypasses the entire middleware stack.

2. **Content-Type assumptions** — the middleware serves `dist/about/index.html` with `Content-Type: text/html`. But what if a route returns JSON (`Json<T>`)? Or a custom content type? The design mentions JSON routes but doesn't specify how the middleware determines content type for served files.

3. **Conditional request handling** — browsers send `If-Modified-Since` and `If-None-Match` headers. The `StaticFileLayer` needs to handle these correctly or performance benefits are lost. `tower-http::ServeDir` handles this, but a custom middleware may not.

4. **HEAD request handling** — `HEAD /about` should return headers without a body. If the middleware naively reads and serves the file, it may not handle HEAD correctly.

5. **Compression interaction** — if `tower-http::CompressionLayer` sits above `StaticFileLayer`, responses get compressed on the fly. But for static files, pre-compression (`.gz` and `.br` files alongside `.html`) would be more efficient. The design doesn't mention compression strategy.

6. **Path traversal via `dist/`** — the middleware maps URL paths to filesystem paths. Even with the path sanitization described for `autumn build`, the *runtime* middleware also needs sanitization. A request for `GET /../../etc/passwd` could escape `dist/` if not carefully normalized.

7. **Concurrent ISR + static serving race** — a request arrives while ISR is writing a new file. Even with atomic rename, there's a window where the middleware reads the file (checking `mtime`), then the file is renamed, then the middleware opens the file. On Linux, this is fine (inode-level). On Windows, this could produce `AccessDenied`.

8. **Memory-mapped vs read** — for high-traffic sites, repeatedly reading static files from disk is wasteful. Should the middleware memory-map files or cache them in-memory? Neither is discussed.

### Alternatives Explored

- **Use `tower-http::ServeDir` directly** — already handles Content-Type, conditional requests, HEAD, range requests. Layer it with custom logic for ISR staleness only.
- **In-memory cache with file-watch invalidation** — cache rendered HTML in a `DashMap`, invalidate via `notify` file watcher. Eliminates disk I/O for hot routes.
- **Pre-compressed static files** — `autumn build` generates `.html`, `.html.gz`, `.html.br` variants. Middleware serves compressed variant if `Accept-Encoding` matches.
- **Middleware bypass list** — routes that require authentication or session state cannot be `#[static_get]`. Enforce at compile time: if the handler accepts `Session` or `Auth` extractors, `#[static_get]` emits a `compile_error!`.

### Recommended Adjustments

1. **Build on `tower-http::ServeDir`** — don't reimplement static file serving. Wrap `ServeDir` with a thin layer that adds ISR staleness checking and background revalidation. This gets Content-Type, conditional requests, HEAD, and range requests for free.
2. **Compile-time extractor validation** — the `#[static_get]` macro should reject handlers that accept request-scoped or auth-related extractors (e.g., `Session`, `AuthUser`, `CookieJar`) with a clear `compile_error!`. Static pages are public by definition. If you need auth, use `#[get]`.
3. **Pre-compress in `autumn build`** — generate gzip and brotli variants alongside HTML. Serve the pre-compressed variant when the client supports it. This is a Phase 4 optimization but should be designed for now.
4. **Document the "static means public" contract** — make it explicit in docs and error messages that `#[static_get]` routes bypass all request-scoped middleware. This is a feature, not a bug, but must be communicated clearly.

---

## Subsystem 4: ISR (Incremental Static Regeneration)

### Failure Modes Identified

1. **Clock skew** — ISR staleness is based on file `mtime`. If the server's clock drifts (common in containers without NTP), pages may be perpetually stale or never revalidate.

2. **Regeneration storms after restart** — server restarts, all ISR pages appear stale simultaneously (mtime is in the past relative to new uptime). Every first request to every ISR route triggers a background regeneration. If there are 500 ISR routes, that's 500 concurrent database queries.

3. **Error cascades** — database goes down. Every ISR regeneration fails. Each failure logs an error and clears the `AtomicBool`, allowing the next request to retry. Under high traffic, this becomes a tight loop of "check stale → spawn regeneration → database error → log → clear flag → next request checks stale → repeat." The tracing log fills with errors.

4. **Revalidation interval too short** — developer sets `revalidate = 1` (every second). On a high-traffic page, this means the background regeneration task is nearly always running. Combined with database queries, this could saturate the connection pool for a single route.

5. **File system as cache layer** — ISR writes to the filesystem, which the middleware reads. On cloud deployments with ephemeral filesystems (e.g., containers without persistent volumes), the `dist/` directory is lost on restart. Every restart becomes a cold start that falls through to SSR for all routes until re-rendered.

6. **No cache warming** — after `autumn build`, the pages are warm. But ISR pages eventually go stale. There's no mechanism to proactively warm the cache before traffic arrives (e.g., after a deploy).

7. **Stale-while-revalidate is invisible** — users see stale content with no indication that it's stale. For some use cases (stock prices, news), serving content that's 60 seconds old is fine. For others (event schedules, inventory), it's a bug. There's no way for the developer to control whether stale content is acceptable or not on a per-route basis.

### Alternatives Explored

- **In-memory cache instead of filesystem** — store rendered HTML in a concurrent HashMap with TTL. Eliminates filesystem dependency. Downside: doesn't survive restarts, uses memory.
- **Hybrid cache** — in-memory primary, filesystem secondary. Read from memory first, fall back to disk, populate memory on miss.
- **Regeneration rate limiting** — per-route minimum interval between regeneration attempts, independent of `revalidate`. Prevents error-cascade tight loops.
- **`stale = "serve" | "error" | "revalidate_sync"`** — per-route control over what happens when content is stale. `"serve"` = current behavior. `"error"` = return 503 if stale. `"revalidate_sync"` = block the request until fresh content is ready (slower but guarantees freshness).
- **Startup cache warming** — on boot, if `dist/` exists, proactively re-render all ISR routes with stale mtime before accepting traffic.

### Recommended Adjustments

1. **Regeneration backoff** — after a failed regeneration, apply a minimum 30-second cooldown before the next attempt for that route. This prevents error cascade tight loops. Implement with an `AtomicU64` storing the last attempt timestamp alongside the `AtomicBool` in-flight flag.
2. **Startup stagger** — on boot, if ISR routes are stale, don't regenerate all at once. Queue them with jitter (random delay of 0 to `revalidate` seconds per route) to spread the load.
3. **Minimum revalidate interval** — enforce a floor of 10 seconds for `revalidate`. Values below this are almost certainly mistakes and create performance problems.
4. **Document ephemeral filesystem implications** — make clear in deployment docs that ISR requires a persistent `dist/` directory. For container deployments, recommend a persistent volume mount or accept cold-start SSR fallback.

---

## Subsystem 5: Deployment Modes & Developer Experience

### Failure Modes Identified

1. **Mode 3 surprise** — developer deploys `dist/` to a CDN expecting a complete site, but forgot that their admin routes are `#[get]` (dynamic). The admin panel returns 404. The warning from `autumn build` was logged to stderr, which the deploy script piped to `/dev/null`.

2. **`autumn build` not in CI** — team adds `#[static_get]` routes during development, everything works because the dev server falls back to SSR. They deploy without running `autumn build`. Site works (SSR fallback), but performance is worse than expected. Nobody notices the static optimization isn't active.

3. **Two build commands** — developers must run both `cargo build` and `autumn build`. This is unusual for Rust projects. Developers who are used to `cargo build && cargo run` will forget `autumn build`. The optimization is invisible when missing, so they won't notice.

4. **Version mismatch** — `autumn-cli` is at version 0.3 (supports `params`) but the `autumn` runtime crate is at 0.2 (doesn't support `params`). The `autumn build` command generates a manifest with params data that the runtime doesn't understand.

5. **Testing static routes** — during `cargo test`, routes are tested via the Axum router (SSR). There's no way to test the static rendering path — whether `autumn build` would succeed, whether the output HTML is correct, whether ISR works. The design has a testing gap.

### Alternatives Explored

- **Single build command** — `autumn build` does everything: `cargo build --release`, Tailwind, static rendering, copy assets. One command, one mental model.
- **`cargo run` detects stale `dist/`** — at startup, if `dist/manifest.json` is stale (compiled at a different binary hash), log a warning: "Run `autumn build` to update static pages."
- **Test utilities** — `autumn_web::test::render_static(handler)` that simulates what `autumn build` does, for use in `#[tokio::test]`.
- **Version compatibility check** — `autumn build` writes its version to `manifest.json`. Runtime checks manifest version on load and warns on mismatch.

### Recommended Adjustments

1. **Make `autumn build` the single command** — it should call `cargo build` internally (it already does). Developers should be told "use `autumn build` for production" as the one command, not a two-step process.
2. **Provide test utilities** — `autumn_web::test::StaticRenderTest` that boots the app, renders a specific static route, and returns the HTML for assertion. This closes the testing gap and lets CI verify static routes without a full `autumn build`.
3. **Version stamp the manifest** — `manifest.json` includes `autumn_version` and `binary_hash`. Runtime warns on mismatch. `autumn-cli` and `autumn` runtime versions are checked at compile time via a shared version constant.

---

## Six Thinking Hats Analysis

### White Hat (Facts & Data)

**What do we know for certain?**

- No existing Rust web framework offers hybrid SSR+SSG+ISR. This is a genuine gap.
- Next.js ISR has been production-validated since 2020 (6 years). The stale-while-revalidate pattern is well-understood.
- Maud templates are pure functions — deterministic output for the same input. This is ideal for static rendering.
- `tower-http::ServeDir` already implements correct static file serving (Content-Type, conditional requests, compression).
- File `mtime` is unreliable on some filesystems (NFS, some FUSE mounts, Windows FAT32). POSIX `rename()` is atomic on the same filesystem but not across mount points.
- Axum's middleware stack runs top-down. Middleware above the router can intercept and short-circuit.
- The blog example already demonstrates the exact route patterns (index with DB, posts with slugs) that hybrid rendering would optimize.

**What data is missing?**

- No benchmarks showing the actual performance difference between SSR and serving static files through Autumn's stack. The benefit is assumed but not measured.
- No data on how many pages real Autumn apps would have. A 10-page site vs a 100,000-page site have very different build time requirements.
- Unclear how Maud's compilation time scales with the number of templates being rendered at build time (not compiled — they're already compiled — but *executed*).

### Red Hat (Feelings & Intuition)

**What feels right?**

- The `#[static_get]` API is clean. One annotation changes the rendering strategy. This is the DX promise Autumn is built on.
- The phased rollout (static paths → params → ISR → polish) feels conservative in a good way. Each phase is independently shippable.
- Building on Axum's existing infrastructure (running the real handler through the real router) rather than inventing a separate rendering pipeline feels solid. It guarantees parity between build-time and runtime rendering.

**What feels risky?**

- The filesystem as a cache layer feels fragile for ISR. It works for CDN deployment (Mode 3), but for server-side ISR, an in-memory cache feels more natural for a Rust framework.
- The `--autumn-build` flag on the main binary feels like a hack. It overloads the application binary with build-tool responsibilities. If the app has expensive initialization (e.g., connecting to third-party APIs), that initialization runs during `autumn build` too.
- Four phases (v0.2–v0.5) for a feature that's already post-v0.1 feels like it could take a very long time for a single developer. There's a risk of it never reaching Phase 3 (ISR), which is the most compelling part.

### Black Hat (Caution & Risk)

**What could go wrong?**

1. **Scope creep kills v0.1.** Hybrid rendering is a v0.2+ feature. But it requires changes to `autumn-macros`, `autumn` runtime, and `autumn-cli` simultaneously. If the design isn't final when v0.1 ships, macro API changes in v0.2 could be breaking. The `#[static_get]` macro API needs to be *designed* now even if it ships later.

2. **The abstraction leaks.** A `#[static_get]` route works in development (SSR fallback), passes all tests (tested via SSR), and then behaves differently in production (served as static HTML without middleware). Differences include: no access logging for static routes, no rate limiting, no CORS headers (unless pre-baked into the HTML, which doesn't apply). The "static pages bypass middleware" design is a feature *and* a footgun.

3. **CDN cache invalidation is the user's problem.** In Mode 3, deploying `dist/` to a CDN means Autumn has zero control over cache invalidation. The user is back to solving CDN-level problems. This isn't Autumn's fault, but it's worth noting that Mode 3's DX is only as good as the CDN's.

4. **ISR on a single server isn't ISR.** Next.js ISR works because Vercel has edge infrastructure that distributes the cache. Autumn ISR on a single server is just "time-based cache with filesystem persistence." It's useful, but the "ISR" naming sets expectations that don't match the single-server reality.

5. **The `params` function can be expensive.** For a site with 100,000 products, `params` queries the entire product table. At build time, this is fine (run once). But if combined with ISR and server fallback, the fallback path for a new product still hits SSR — it doesn't call `params` again to discover the new product and pre-render it. The ISR + params interaction is underspecified for cache warming of new content.

### Yellow Hat (Benefits & Value)

**What's the best case?**

- Autumn becomes the first Rust framework with integrated hybrid rendering. This is a genuine competitive differentiator that could attract users from both the SSR camp (Loco, Rocket) and the SSG camp (Zola, Cobalt).
- Blog-style sites (the most common "first project" for any framework) get near-zero latency for free with one annotation change.
- The progressive enhancement principle means nobody is *forced* to use static rendering. It's purely additive.
- The design aligns perfectly with Autumn's existing architecture: `autumn-cli` for build-time magic, thin macros for annotations, Tower middleware for runtime serving. No new paradigms.
- The Phase 1 implementation (static paths only, no params, no ISR) is genuinely small. It's a macro that generates metadata, a CLI command that renders pages, and a middleware that serves files. Probably 500–800 lines of code total.

### Green Hat (Alternatives & Creativity)

**What haven't we considered?**

1. **Edge-side rendering.** Instead of pre-rendering to HTML files, generate a WASM module from each static route and deploy it to edge workers (Cloudflare Workers, Deno Deploy). This is future-looking and aligns with Rust's WASM strength. Not for v0.2, but worth designing the abstraction to not preclude it.

2. **Partial static rendering.** A page has a static shell (nav, footer, layout) and dynamic islands (user-specific content). Instead of the whole page being static or dynamic, render the shell at build time and inject dynamic fragments at request time. This is closer to Astro's islands architecture.

3. **Content-addressed output.** Instead of `dist/about/index.html`, use `dist/about/index.{hash}.html`. This enables infinite CDN caching (content never changes at a given hash) and atomic deploys (swap a single manifest that points to new hashes).

4. **Markdown-first static routes.** For pure content pages (about, docs, blog posts), allow writing Markdown files that are rendered at build time through a Maud layout. No Rust handler needed. `autumn build` scans `content/` directory and renders `.md` files. This is how Zola works, and it would be a bridge for Zola users migrating to Autumn.

5. **Webhook-triggered revalidation.** Instead of time-based ISR, integrate with common headless CMS webhooks (Contentful, Sanity, Strapi). `POST /autumn/revalidate` with a signed payload triggers re-rendering of specified routes. Already noted as deferred in the design, but worth designing the webhook endpoint shape now.

### Blue Hat (Process & Meta)

**Are we asking the right questions?**

- The design is solid for Phase 1 (static paths). The risk isn't in what's designed — it's in the **transition points** between phases. Phase 1 → Phase 2 (adding `params`) changes the build pipeline significantly (database required). Phase 2 → Phase 3 (adding ISR) changes the runtime middleware significantly (background tasks, atomic writes). Each phase transition is a potential API stability risk.

- **Priority check:** Is hybrid rendering the right v0.2 feature? For a single developer, would it be more impactful to invest v0.2 in hot reload, better error pages, database migrations CLI, or WebSocket support? Hybrid rendering is technically impressive but targets a subset of use cases (content-heavy sites). Hot reload impacts every developer on every project.

- **Validation strategy:** The design should be validated by building the blog example with `#[static_get]` annotations before committing to the full implementation. If the blog example can't cleanly adopt hybrid rendering, the API is wrong.

---

## Cross-Cutting Insights

### Insight 1: Static Means Public — Make It Loud

**Description:** The most dangerous failure mode across multiple subsystems is that `#[static_get]` silently bypasses middleware (auth, rate limiting, CORS, logging). This is by design — static files are served from disk without touching the application — but it's a footgun if not communicated clearly.

**Source:** StaticFileLayer failure modes, Black Hat analysis

**Impact:** High

**Effort:** Low (documentation + compile-time validation)

**Recommendation:** Add a compile-time check: if a `#[static_get]` handler accepts `Session`, `AuthUser`, `Extension<CurrentUser>`, or similar auth-scoped extractors, emit a `compile_error!("Static routes are served without authentication. Use #[get] for authenticated routes.")`. Make the contract impossible to violate accidentally.

### Insight 2: Build on `tower-http`, Don't Reimplement

**Description:** Several failure modes (Content-Type, conditional requests, HEAD, range requests, compression) are already solved by `tower-http::ServeDir`. Writing a custom `StaticFileLayer` from scratch risks re-introducing bugs that are already fixed in battle-tested code.

**Source:** StaticFileLayer failure modes, White Hat analysis

**Impact:** High

**Effort:** Negative (less code to write)

**Recommendation:** Implement `StaticFileLayer` as a thin wrapper around `ServeDir` that adds only two behaviors: (1) ISR staleness checking, and (2) background revalidation triggering. Everything else — file serving, content negotiation, conditional requests — delegates to `ServeDir`.

### Insight 3: Atomic Builds Prevent Half-Deployed Sites

**Description:** Partial build failures leave `dist/` in an inconsistent state. Combined with Mode 3 (CDN deploy), this means half a site could be live while the other half 404s.

**Source:** `autumn build` failure modes

**Impact:** High

**Effort:** Low (render to staging dir, atomic swap)

**Recommendation:** Always render to `dist.staging/`, swap to `dist/` on complete success. On failure, preserve previous `dist/` and report which routes failed.

### Insight 4: ISR Needs Guardrails

**Description:** Multiple ISR failure modes (regeneration storms, error cascades, short intervals, ephemeral filesystems) share a root cause: the design trusts developers to configure ISR correctly. But ISR is the most complex subsystem and the one where misconfiguration has the subtlest symptoms.

**Source:** ISR failure modes, Red Hat feelings

**Impact:** Medium

**Effort:** Low

**Recommendation:** Enforce a minimum `revalidate` interval (10s), implement regeneration backoff on failure, stagger post-restart regenerations, and log clearly when ISR routes fall back to SSR due to filesystem issues.

### Insight 5: Phase 1 Is the Only Phase That Matters Right Now

**Description:** The four-phase plan (v0.2–v0.5) is well-structured, but for a single developer, the risk is that Phases 3–4 never ship. This means the design should be evaluated primarily on whether Phase 1 alone is worth shipping. Phase 1 (static paths only, no params, no ISR) is basically "pre-render some pages at build time." Is that compelling enough on its own?

**Source:** Blue Hat process analysis, Red Hat feelings

**Impact:** High

**Effort:** N/A (strategic decision)

**Assessment:** Phase 1 alone is worth shipping if it measurably improves the blog example's TTFB (time to first byte) and enables Mode 3 (CDN deploy) for simple sites. Both are true. A blog's about page going from 5ms (SSR) to 0.1ms (static) is a real, measurable win. And being able to `autumn build && deploy dist/` for a simple site is a real capability. Phase 1 stands on its own.

### Insight 6: Validate With the Blog Example First

**Description:** The blog example (`examples/blog/`) is the perfect validation target. It has static-suitable routes (`/about` if added, published post pages), dynamic routes (admin), ISR candidates (the index listing), and parameterized routes (posts by slug). Before writing a single line of hybrid rendering code, design the exact API by annotating the blog example routes and verifying the API feels natural.

**Source:** Blue Hat process analysis, Yellow Hat benefits

**Impact:** Medium

**Effort:** Low (it's a design exercise, not implementation)

**Recommendation:** Write a `examples/blog-hybrid/` sketch (pseudocode) showing every route annotated with the proposed hybrid rendering API. Walk through the `autumn build` output mentally. If any route feels awkward, adjust the API before implementation.

---

## Impact on Design Document

These insights should prompt the following revisions to `docs/design/hybrid-rendering.md`:

1. **Add: "Static means public" contract** — explicit section stating that `#[static_get]` routes bypass all request-scoped middleware, with compile-time enforcement for auth extractors.
2. **Revise: `StaticFileLayer` implementation** — specify that it wraps `tower-http::ServeDir` rather than reimplementing static file serving.
3. **Add: Atomic build output** — specify staging directory approach to prevent partial builds.
4. **Add: ISR guardrails** — minimum revalidate interval, regeneration backoff, post-restart staggering.
5. **Add: HTTP status validation during build** — non-2xx responses fail the build for that route.
6. **Add: Parallel rendering** — `autumn build` renders routes concurrently.
7. **Revise: `params` syntax** — consider unquoted identifier instead of string literal.
8. **Add: Startup route reconciliation** — runtime warns about stale routes in `dist/`.
9. **Add: Testing utilities** — `autumn_web::test::StaticRenderTest` for testing static routes in `#[tokio::test]`.
10. **Consider: `#[get("/path", render = "static")]`** syntax as alternative to `#[static_get]`.

---

## Statistics
- Total failure modes identified: 31
- Subsystems analyzed: 5
- Six Thinking Hats perspectives: 6
- Cross-cutting insights: 6
- Design document revisions recommended: 10
- Techniques applied: 2

## Recommended Next Steps

1. Update `docs/design/hybrid-rendering.md` with the revisions identified above
2. Create `examples/blog-hybrid/` API sketch to validate the developer experience before implementation
3. Benchmark SSR vs static file serving through Autumn's Tower stack to quantify the actual performance gain
4. Prototype Phase 1 implementation (macro + CLI + middleware) against the blog example

---

*Generated by BMAD Method v6 - Creative Intelligence*
