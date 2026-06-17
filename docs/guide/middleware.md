# Middleware in Autumn

Autumn ships a curated stack of built-in middleware — request IDs, security
headers, CSRF, CORS, sessions, metrics, exception filters. That covers the
boring-but-critical concerns most applications share. When you need something
off the beaten path (a timeout, a rate limiter, a custom tracing span, a
legacy header injector), reach for [`AppBuilder::layer`] and drop in any
standard [`tower::Layer`].

This guide explains where user layers sit in the stack, how to register them,
and the common recipes.

---

## Quick start

Apply a Tower timeout layer to every route in the app:

```rust,no_run
use std::time::Duration;
use autumn_web::prelude::*;
use axum::{error_handling::HandleErrorLayer, http::StatusCode};
use tower::{ServiceBuilder, timeout::TimeoutLayer};

#[get("/slow")]
async fn slow() -> &'static str {
    tokio::time::sleep(Duration::from_secs(10)).await;
    "done"
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![slow])
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|_| async {
                    StatusCode::REQUEST_TIMEOUT
                }))
                .layer(TimeoutLayer::new(Duration::from_secs(5))),
        )
        .run()
        .await;
}
```

Tower's `TimeoutLayer` surfaces its own `BoxError` on timeout, while axum
requires every layer to produce `Infallible`. `HandleErrorLayer` bridges the
two — it converts any error from the inner layer into an HTTP response.

---

## Middleware ordering

On a request's **ingress** path (outermost → innermost), layers run in this
order:

```
  AccessLog (fallback)
    └─ Metrics
         └─ ExceptionFilter
              └─ ErrorPageContext
                   └─ Session
                        └─ SecurityHeaders
                             └─ RequestId
                                  └─ LogContext
                                       └─ AccessLog (primary)
                                            └─ [your .layer() calls, first = outermost]
                                                 └─ CSRF
                                                      └─ CORS
                                                           └─ route handler
```

`LogContext` establishes the request-scoped log context (request id
correlation for every log line); it sits inside `RequestId` so the id is
always available, and outside your layers so events they emit are correlated.
The structured per-request access line (`autumn::access`) is emitted by the
**primary** `AccessLog` layer just inside `LogContext`, so the line is
correlated to the request span and carries the request id. Responses that
short-circuit above it — session-store outages, and in production startup
503s, pre-built static page hits, and the MCP endpoint — are caught by the
outermost **fallback** `AccessLog`, which logs them with the wire status (and
without a request id, since `RequestIdLayer` never ran for them).

The ordering guarantee that matters most: **user layers run inside
`RequestIdLayer` on ingress**, so every `.layer()` you register can read the
generated `RequestId` from the request extensions. Exception filters,
metrics, and error-page rendering all sit *outside* your layers, which means
errors you produce (and errors you let bubble up from handlers) are still
caught by Autumn's error pipeline.

Multiple `.layer()` calls stack in registration order, mirroring
[`tower::ServiceBuilder`]: the first `.layer(A)` call becomes the outermost
user layer, so `A` sees the request first and the response last.

---

## Wrap shared state in `Arc`

Because `AppBuilder::layer()` requires the layer to be `Clone + Send + Sync +
'static`, any state your middleware needs to share across requests — HTTP
client pools, metrics registries, rate-limit stores, caches — should live
behind an [`Arc`]. Clone the layer; the `Arc` cheaply bumps a refcount.

```rust,ignore
use std::sync::Arc;

#[derive(Clone)]
struct MetricsLayer {
    registry: Arc<prometheus::Registry>, // shared, cheaply clonable
}
```

Trying to store the raw `prometheus::Registry` directly would force every
request-handling clone to deep-copy the registry (if it were `Clone` at all)
and would fail the `Sync` bound outright for types like `RefCell`. `Arc`
sidesteps both issues.

## Reading the request ID from a custom layer

```rust,ignore
use autumn_web::middleware::RequestId;
use axum::http::Request;

fn log_with_id<B>(req: &Request<B>) {
    if let Some(id) = req.extensions().get::<RequestId>() {
        tracing::info!(request_id = %id, "custom layer fired");
    }
}
```

Because user layers sit inside `RequestIdLayer`, the extension is always
present in `call(..)` — there's no race condition to worry about.

---

## Gating cached pages with `static_gate`

When you pre-render routes (SSG) or revalidate them on a schedule (ISG), the
cached HTML is served by Autumn's static-first middleware **before** the inner
router — session, auth, and your `.layer()` calls — is ever reached. That is
what makes static hits fast and keeps them available even if the session
backend is down, but it also means the framework's auth layers cannot gate a
pre-rendered response: the same HTML is served to every visitor regardless of
auth state.

`AppBuilder::static_gate` is Autumn's answer to this, analogous to Next.js
*Edge Middleware* (`middleware.ts`) running before the CDN cache lookup. A gate
layer runs **outermost** — outside the session layer and ahead of the static
cache — so it can redirect or reject a request before a cached page is served:

```
static_gate (auth check / redirect)
  └─ static cache lookup
       └─ pre-rendered page served (or regenerated for ISG)
            └─ … session, your .layer() calls, route handler …
```

```rust,ignore
use autumn_web::prelude::*;
use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::Response,
};

async fn require_auth(req: Request, next: Next) -> Response {
    // Only gate page navigation: let non-GET/HEAD requests (JSON APIs, form
    // POSTs, the `/mcp` JSON-RPC transport, CORS preflights) pass through so a
    // browser redirect never turns them into a 302.
    let is_page = matches!(req.method(), &Method::GET | &Method::HEAD);
    // Verify a signed/JWT session cookie DIRECTLY — the session Extension is
    // not available this far out in the stack.
    if !is_page || has_valid_session_cookie(req.headers()) {
        next.run(req).await
    } else {
        Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "/login")
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

autumn_web::app()
    .routes(routes![dashboard])
    .static_gate(axum::middleware::from_fn(require_auth))
    .run()
    .await;
```

Key properties and trade-offs:

- **Runs before the static cache** in SSG/ISG mode, so cached pages can be
  auth-gated without baking user-specific content into the pre-rendered HTML.
- **Runs in the same outermost position in fully-dynamic mode** (no `dist/`
  directory), so the same gate behaves identically whether or not static
  generation is active — gating code is portable.
- **No session `Extension`.** The session layer runs *inside* the gate, so you
  cannot read session-populated extensions here. Verify a signed session cookie
  or JWT directly, using the same signing key you configure for sessions.
- **Personalised content still needs a dynamic route** (or client-side fetch).
  `static_gate` decides *whether* to serve a cached page, not *what* it
  contains.
- **Page-cache gate, not API auth.** The gate is global, so a well-behaved gate
  should no-op on non-GET/HEAD requests (note the `is_page` check above) — a
  browser redirect is meaningless for a JSON API or the `/mcp` JSON-RPC POST
  transport, and the gate is never applied to MCP `tools/call` dispatch anyway.
  Authenticate JSON APIs and MCP tools with route-level guards / `#[secured]` /
  session auth.
- Multiple `static_gate` calls stack in registration order (first =
  outermost), like `.layer()`. Plugins can pre-flight with
  `has_static_gate::<L>()` / `get_static_gate_types()`.

---

## Limitations (for now)

- **No per-route layers.** `.layer()` wraps the whole app. If you need a
  middleware scoped to a group of routes, use
  [`AppBuilder::scoped`] — it accepts the same `tower::Layer` bounds and
  applies the layer only to the routes in that group. Per-route layering
  (equivalent to axum's `route_layer`) is tracked as a follow-up.
- **`Service::Error = Infallible`.** Any layer you register must produce
  `Infallible` on its service's `Error` associated type. For layers that
  surface real errors (timeouts, rate limits, circuit breakers), wrap them
  with [`axum::error_handling::HandleErrorLayer`] as shown above.

---

## Recipes

### Rate limiting with `tower-governor`

```rust,ignore
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};

let governor_conf = GovernorConfigBuilder::default()
    .per_second(10)
    .burst_size(20)
    .finish()
    .unwrap();

autumn_web::app()
    .routes(routes![index])
    .layer(GovernorLayer::new(governor_conf))
    .run()
    .await;
```

### Extra tracing span per request

```rust,ignore
use tower_http::trace::TraceLayer;

autumn_web::app()
    .routes(routes![index])
    .layer(TraceLayer::new_for_http())
    .run()
    .await;
```

### Custom header injection (legacy system integration)

Write a small `Layer`/`Service` pair (see the pattern in
`autumn/tests/custom_layer.rs`) that rewrites or inserts request/response
headers, then register it with `.layer(MyLayer)`. Because the layer sits
inside `RequestIdLayer`, you can stamp the request ID onto any outgoing
header for downstream services.

---

## See also

- [`AppBuilder::layer`] — method reference and trait bounds.
- [`AppBuilder::scoped`] — the group-scoped variant.
- [Error reporting guide](./error-reporting.md) — catch handler panics and ship
  panics + 5xx errors to a pluggable reporter (Sentry/Slack/custom). The
  panic-aware promotion of the `ExceptionFilter` concept shown in the ordering
  diagram above.
- [Extensibility guide](./extensibility.md) — picks the right tier for your
  extension point.

[`AppBuilder::layer`]: https://docs.rs/autumn-web/latest/autumn_web/app/struct.AppBuilder.html#method.layer
[`AppBuilder::scoped`]: https://docs.rs/autumn-web/latest/autumn_web/app/struct.AppBuilder.html#method.scoped
[`tower::Layer`]: https://docs.rs/tower/latest/tower/trait.Layer.html
[`tower::ServiceBuilder`]: https://docs.rs/tower/latest/tower/struct.ServiceBuilder.html
[`axum::error_handling::HandleErrorLayer`]: https://docs.rs/axum/latest/axum/error_handling/struct.HandleErrorLayer.html

---

## Forwarded-header client identity (plugin author guidance)

When writing middleware that needs the real client IP, hostname, or scheme,
**never read `X-Forwarded-*` headers directly.** Direct reads are fragile,
bypass the operator's trust policy, and can introduce SSRF / IP-spoofing
vulnerabilities. Use the blessed extractors instead:

| Extractor | What it resolves |
|-----------|-----------------|
| `ClientAddr` | Real client IP after trust evaluation |
| `ClientHost` | External host (`X-Forwarded-Host` or `Host`) |
| `ClientScheme` | External scheme (`X-Forwarded-Proto` or URI scheme) |

```rust,no_run
use autumn_web::extract::{ClientAddr, ClientHost, ClientScheme};
use autumn_web::prelude::*;

#[get("/info")]
async fn info(
    ClientAddr(ip): ClientAddr,
    ClientHost(host): ClientHost,
    ClientScheme(scheme): ClientScheme,
) -> String {
    format!("client={ip} host={host} scheme={scheme}")
}
```

The values are resolved once per request by the framework's
`TrustedProxiesLayer`, using the operator's `[security.trusted_proxies]`
configuration. Middleware written inside the framework stack can read
`ResolvedClientIdentity` directly from request extensions:

```rust,no_run
use autumn_web::security::ResolvedClientIdentity;

// Inside a Tower Service::call:
let identity = req.extensions().get::<ResolvedClientIdentity>();
```

See [`security.trusted_proxies` configuration](../guide/getting-started.md)
for operator setup instructions.
