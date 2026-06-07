# Health Indicators

Autumn's `/actuator/health` and `/ready` endpoints surface the framework's own
state (startup, graceful shutdown, DB pool stats). The `HealthIndicator` trait
lets any component — a payment gateway, a feature-flag service, an SMTP relay,
a downstream HTTP API — **plug its own health check** into those same endpoints.

This mirrors Spring Boot's `HealthIndicator` model. Autumn adapts it for async
Rust: the `check()` method returns a `BoxFuture`, every indicator runs with a
per-indicator timeout (default 2 s), and registration is explicit via
`AppBuilder`.

---

## Quick start

### 1. Implement `HealthIndicator`

```rust
use autumn_web::actuator::{HealthCheckOutput, HealthIndicator, HealthStatus};
use std::collections::HashMap;

pub struct StripeIndicator {
    // your HTTP client, config, etc.
}

impl HealthIndicator for StripeIndicator {
    fn check(&self) -> futures::future::BoxFuture<'_, HealthCheckOutput> {
        Box::pin(async move {
            // Try a lightweight Stripe API call (e.g. list balance)
            match self.ping_stripe().await {
                Ok(_) => HealthCheckOutput::up(),
                Err(e) => {
                    let mut details = HashMap::new();
                    details.insert("error".to_string(), serde_json::json!(e.to_string()));
                    HealthCheckOutput {
                        status: HealthStatus::Down,
                        details,
                    }
                }
            }
        })
    }
}
```

### 2. Register with `AppBuilder`

```rust
use std::sync::Arc;

autumn_web::app()
    .routes(routes![...])
    .health_indicator("stripe", Arc::new(StripeIndicator::new()))
    .run()
    .await;
```

### 3. Verify it appears in `/actuator/health`

```bash
curl http://localhost:3000/actuator/health | jq .
```

```json
{
  "status": "UP",
  "version": "0.5.0",
  "profile": "dev",
  "uptime": "12s",
  "components": {
    "stripe": {
      "status": "UP"
    }
  }
}
```

---

## Status precedence

Overall status follows Spring Boot precedence (most-severe wins):

| Condition | Overall status | HTTP code |
|-----------|---------------|-----------|
| Any indicator is `DOWN` | `DOWN` | 503 |
| Any `OUT_OF_SERVICE`, no `DOWN` | `OUT_OF_SERVICE` | 503 |
| Any `UNKNOWN`, no failures | `UNKNOWN` | 200 |
| All `UP` (or no indicators) | `UP` | 200 |

The built-in DB pool check participates in the same aggregation.

---

## Readiness vs health-only

By default an indicator gates **both** `/ready` and `/actuator/health`
(`IndicatorGroup::Readiness`). A Kubernetes deploy is blocked until the
indicator is healthy.

To contribute to `/actuator/health` only — without blocking rolling deploys —
override the `group()` method:

```rust
use autumn_web::actuator::IndicatorGroup;

impl HealthIndicator for StripeIndicator {
    fn group(&self) -> IndicatorGroup {
        IndicatorGroup::HealthOnly   // does not gate /ready
    }
    // ...
}
```

**When to use `HealthOnly`**: payment gateways, analytics sinks, non-critical
notification services. A degraded Stripe connection doesn't mean the app can't
serve requests.

**When to use `Readiness`** (default): databases your app can't function
without, feature-flag services that gate core flows, cache layers whose absence
causes unacceptable degradation.

---

## Per-indicator timeout

Each indicator runs with a timeout. If `check()` does not resolve in time the
indicator is reported as `UNKNOWN` with `timed_out: true` in its `details`.
The default is 2 000 ms. Override it per-indicator:

```rust
impl HealthIndicator for SlowExternalService {
    fn timeout_ms(&self) -> u64 { 5_000 }   // 5 s for a slow upstream

    fn check(&self) -> futures::future::BoxFuture<'_, HealthCheckOutput> {
        Box::pin(async move { /* ... */ })
    }
}
```

A timed-out indicator in the `/actuator/health` response:

```json
{
  "components": {
    "slow_service": {
      "status": "UNKNOWN",
      "details": { "timed_out": true }
    }
  }
}
```

---

## Hiding details in production

When `health.detailed = false` (the default in `prod` profile), the
per-component `details` map is **omitted** from the response. The `status`
field is always present.

```toml
# autumn-prod.toml
[health]
detailed = false
```

---

## Registering from a plugin

Plugins use the same `AppBuilder` API inside their `build()` method:

```rust
use autumn_web::plugin::Plugin;
use autumn_web::app::AppBuilder;
use std::sync::Arc;

pub struct PaymentsPlugin { /* ... */ }

impl Plugin for PaymentsPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        app.health_indicator("payments", Arc::new(PaymentsHealthIndicator::new()))
    }
}
```

This means `autumn-admin-plugin` or any future plugin can contribute health
indicators without requiring app glue code.

---

## Built-in indicators

| Name | Feature flag | Group | What it checks |
|------|-------------|-------|----------------|
| `db` | `db` | Readiness | DB connection pool availability |

The `db` indicator replaces the previous hard-coded pool check. Its output
appears in both the new `components.db` key and the legacy `checks.database`
key for backwards compatibility.
