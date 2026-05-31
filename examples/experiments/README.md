# A/B Experiments Example

Demonstrates Autumn's built-in A/B experiment system: deterministic bucketing,
sticky assignments, exposure telemetry, staff/QA overrides, and the experiment
lifecycle.

## Prerequisites

- Rust 1.88.0+

## Quick start

```sh
cargo run -p experiments
```

Then in a second terminal:

```sh
# Assign user:1 — variant printed + exposure logged to stdout
curl http://localhost:3000/checkout/user:1

# Same actor always gets the same variant (sticky)
curl http://localhost:3000/checkout/user:1

# Different actors may get different variants
curl http://localhost:3000/checkout/user:2

# QA actor is overridden to "treatment" regardless of bucket
curl http://localhost:3000/checkout/qa:alice

# Check current experiment status
curl http://localhost:3000/status

# Conclude the experiment (returns winner for everyone)
curl -X POST http://localhost:3000/conclude
curl http://localhost:3000/checkout/user:99
```

## What it shows

| Feature | Where |
|---------|-------|
| 50/50 variant assignment | `ExperimentConfig::new` with two `VariantConfig`s |
| Deterministic bucketing | FNV-1a hash of `"<experiment>:<actor>"` mod 10 000 |
| Sticky assignments | Same actor always returns the same variant |
| Exposure telemetry | `TracingExposureSink` logs to stdout via `tracing` |
| QA override | `svc.set_override("checkout_v2", "qa:alice", "treatment")` |
| Experiment lifecycle | `draft → running → concluded` via `start()` / `conclude()` |
| Custom sink | Replace `TracingExposureSink` with `SegmentSink`, `PostHogSink`, etc. |

## Key code

```rust
use autumn_web::experiments::{
    ExperimentConfig, ExperimentService, InMemoryExperimentStore, VariantConfig,
};

let svc = ExperimentService::new(Arc::new(InMemoryExperimentStore::new()));

svc.create(ExperimentConfig::new("checkout_v2", vec![
    VariantConfig::new("control",   50),
    VariantConfig::new("treatment", 50),
])).unwrap();

svc.start("checkout_v2").unwrap();

// In a handler:
let variant = svc.assign("checkout_v2", "user:42").unwrap();
```
