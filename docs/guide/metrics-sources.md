# Plugin Metrics Sources

Autumn's `/actuator/prometheus` and `/actuator/metrics` endpoints expose the
framework's built-in `autumn_http_*` metric families.  The `MetricsSource`
trait lets any in-process subsystem — a background-worker plugin, a workflow
engine, an application-level queue — contribute additional families to the
**same scrape endpoint**, with no second port or exporter.

This is the metrics analogue of the pluggable `HealthIndicator` model and
mirrors Spring Boot's `MeterRegistry` pattern.

---

## Quick start

### 1. Implement `MetricsSource`

```rust
use autumn_web::actuator::{MetricFamily, MetricKind, MetricSample, MetricsSource};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct QueueMetrics {
    depth: Arc<AtomicU64>,
}

impl QueueMetrics {
    pub fn new(depth: Arc<AtomicU64>) -> Self {
        Self { depth }
    }
}

impl MetricsSource for QueueMetrics {
    fn collect(&self) -> Vec<MetricFamily> {
        vec![MetricFamily {
            name: "myapp_queue_depth".to_string(),
            help: "Current number of items in the processing queue".to_string(),
            kind: MetricKind::Gauge,
            samples: vec![MetricSample {
                labels: vec![],
                value: self.depth.load(Ordering::Relaxed) as f64,
            }],
        }]
    }
}
```

### 2. Register it with `AppBuilder`

```rust
use std::sync::Arc;

let depth = Arc::new(std::sync::atomic::AtomicU64::new(0));
let metrics = QueueMetrics::new(depth.clone());

autumn_web::app()
    .routes(routes![...])
    .metrics_source("myapp_queue", Arc::new(metrics))
    .run()
    .await;
```

### 3. Scrape the unified endpoint

```
$ curl http://localhost:3000/actuator/prometheus
# HELP autumn_http_requests_total Total number of HTTP requests
# TYPE autumn_http_requests_total counter
autumn_http_requests_total 1234
...
# HELP myapp_queue_depth Current number of items in the processing queue
# TYPE myapp_queue_depth gauge
myapp_queue_depth 42
```

---

## Naming and namespacing rules

- **Prefix every metric name** with a stable namespace that identifies your
  subsystem (e.g. `harvest_` for autumn-harvest, `myapp_` for application-level
  sources, `admin_` for autumn-admin-plugin).  This avoids collisions with the
  built-in `autumn_*` families and other sources.
- **The registration name** passed to `.metrics_source(name, ...)` is the
  stable identifier used for duplicate detection and the error-isolation counter
  label.  Choose something short and unique (e.g. `"myapp_queue"`).
- **Family names** within a source are your responsibility — the registry does
  not enforce cross-source uniqueness of individual `MetricFamily::name` values.

---

## Multi-label samples

`MetricSample::labels` is a `Vec<(String, String)>`, rendered as
`{key="value",...}` in the Prometheus text format.  Label values are
automatically escaped (backslash, newline, double-quote).

```rust
MetricFamily {
    name: "harvest_activity_retries_total".to_string(),
    help: "Activity retries by queue".to_string(),
    kind: MetricKind::Counter,
    samples: vec![
        MetricSample {
            labels: vec![("queue".to_string(), "email".to_string())],
            value: 12.0,
        },
        MetricSample {
            labels: vec![("queue".to_string(), "sms".to_string())],
            value: 3.0,
        },
    ],
}
```

---

## Error isolation

If a `MetricsSource` implementation **panics** during a scrape:

- Its families are **omitted** from that scrape's output — the rest of the
  response is unaffected.
- An `autumn_metrics_source_errors_total{source="<name>"}` counter
  **increments** so you can alert on broken sources.
- The panic is caught and swallowed; it does not propagate to the HTTP handler.

```
# HELP autumn_metrics_source_errors_total Number of scrape errors (panics) per plugin metrics source
# TYPE autumn_metrics_source_errors_total counter
autumn_metrics_source_errors_total{source="my_broken_source"} 1
```

The sync-snapshot contract — `collect` **must not block on I/O** — is
enforced by convention, not by the framework.  If you need async data, collect
it into an `Arc<RwLock<Snapshot>>` and refresh that snapshot from a background
task.

---

## Duplicate registration

Registering two sources with the same name is caught at **startup time**:

```rust
app
    .metrics_source("payments", Arc::new(PaymentsMetrics))
    .metrics_source("payments", Arc::new(PaymentsMetrics)) // ← duplicate
```

The second registration emits a `tracing::warn!` and is **skipped**.  The
same behaviour applies when a plugin calls `.metrics_source` from `Plugin::build`
and the user also calls it directly.

---

## Registering from a plugin

`Plugin::build` receives an `AppBuilder` so plugins can wire a source with no
app-level glue code:

```rust
use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use std::sync::Arc;

pub struct HarvestPlugin { /* ... */ }

impl Plugin for HarvestPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        let metrics = Arc::new(HarvestMetrics::new(/* ... */));
        app
            .on_startup(/* ... */)
            .metrics_source("harvest", metrics)
    }
}
```

Users of the plugin call only:

```rust
autumn_web::app()
    .plugins(HarvestPlugin::new())
    .run()
    .await;
```

---

## JSON endpoint (`/actuator/metrics`)

Plugin-contributed sources also appear in the JSON endpoint under the `sources`
key, alongside the existing top-level HTTP and database keys:

```json
{
  "http": { "requests_total": 1234, ... },
  "sources": {
    "myapp_queue": [
      {
        "name": "myapp_queue_depth",
        "help": "Current number of items in the processing queue",
        "kind": "gauge",
        "samples": [{ "labels": {}, "value": 42.0 }]
      }
    ]
  }
}
```

The existing top-level JSON keys (`http`, `database`) are unchanged —
current scrapers are not affected.

---

## Actuator exposure config

Sources respect the same actuator exposure settings as the built-in families.
If `/actuator/prometheus` is only reachable in sensitive mode (see
`actuator.sensitive` in `autumn.toml`), plugin-contributed families are also
behind that gate — no per-source exposure config is needed.
