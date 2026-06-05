//! A/B experiment example: two-variant checkout flow with exposure events.
//!
//! Demonstrates:
//! - Declaring a named experiment with two 50/50 variants
//! - Deterministic, sticky actor assignment
//! - Exposure events logged to stdout via the default tracing sink
//! - Staff/QA override pinning a specific actor to a variant
//! - Concluding an experiment with a winner
//! - A custom `MetricsSource` that surfaces experiment exposure counts in
//!   `/actuator/prometheus` alongside the built-in `autumn_http_*` families
//!
//! # Quick start
//!
//! ```sh
//! cargo run -p experiments
//! ```
//!
//! Then in a second terminal:
//!
//! ```sh
//! # Assign user:1 — variant printed + exposure logged to stdout
//! curl http://localhost:3000/checkout/user:1
//!
//! # Same actor always gets the same variant (sticky)
//! curl http://localhost:3000/checkout/user:1
//!
//! # Different actors may get different variants
//! curl http://localhost:3000/checkout/user:2
//!
//! # QA actor is overridden to "treatment" regardless of bucket
//! curl http://localhost:3000/checkout/qa:alice
//!
//! # Check current experiment status
//! curl http://localhost:3000/status
//!
//! # See experiment metrics alongside HTTP metrics in one scrape
//! curl http://localhost:3000/actuator/prometheus
//!
//! # Conclude the experiment (returns winner for everyone)
//! curl -X POST http://localhost:3000/conclude
//! curl http://localhost:3000/checkout/user:99
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use autumn_web::actuator::{MetricFamily, MetricKind, MetricSample, MetricsSource};
use autumn_web::experiments::{
    ExperimentConfig, ExperimentService, InMemoryExperimentStore, VariantConfig,
};
use autumn_web::prelude::*;

// ── Custom MetricsSource ──────────────────────────────────────────────────────

/// Reports per-variant assignment counts to `/actuator/prometheus`.
///
/// This is registered via `AppBuilder::metrics_source` in `main` and updates
/// atomically from the HTTP handlers, so `collect` never blocks on I/O.
struct ExperimentMetrics {
    control_assignments: Arc<AtomicU64>,
    treatment_assignments: Arc<AtomicU64>,
}

impl MetricsSource for ExperimentMetrics {
    fn collect(&self) -> Vec<MetricFamily> {
        vec![MetricFamily {
            name: "experiments_assignments_total".to_string(),
            help: "Total variant assignments for the checkout_v2 experiment".to_string(),
            kind: MetricKind::Counter,
            samples: vec![
                MetricSample {
                    labels: vec![("variant".to_string(), "control".to_string())],
                    value: self.control_assignments.load(Ordering::Relaxed) as f64,
                },
                MetricSample {
                    labels: vec![("variant".to_string(), "treatment".to_string())],
                    value: self.treatment_assignments.load(Ordering::Relaxed) as f64,
                },
            ],
        }]
    }
}

const EXPERIMENT: &str = "checkout_v2";

/// Shared assignment counters updated from the checkout handler.
#[derive(Clone)]
struct AssignmentCounters {
    control: Arc<AtomicU64>,
    treatment: Arc<AtomicU64>,
}

/// Assign a variant to the given actor and return it as plain text.
///
/// Exposure events are emitted automatically to tracing (stdout in this example).
/// Assignment counts are also incremented so `ExperimentMetrics::collect` can
/// report them without any blocking I/O.
#[get("/checkout/{actor}")]
async fn checkout(
    Path(actor): Path<String>,
    exps: autumn_web::experiments::Experiments,
    State(state): State<autumn_web::AppState>,
) -> String {
    match exps.service().assign(EXPERIMENT, &actor) {
        Ok(variant) => {
            if let Some(counters) = state.extension::<AssignmentCounters>() {
                match variant.as_str() {
                    "control" => counters.control.fetch_add(1, Ordering::Relaxed),
                    "treatment" => counters.treatment.fetch_add(1, Ordering::Relaxed),
                    _ => 0,
                };
            }
            format!(
                "actor={actor} experiment={EXPERIMENT} variant={variant}\n\
                 (exposure event logged to stdout)\n"
            )
        }
        Err(e) => format!("assignment error: {e}\n"),
    }
}

/// Show the current experiment configuration.
#[get("/status")]
async fn status(exps: autumn_web::experiments::Experiments) -> String {
    match exps.service().status(EXPERIMENT) {
        Ok(cfg) => {
            let variants: String = cfg
                .variants
                .iter()
                .map(|v| format!("  {}={}", v.name, v.weight))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "experiment: {}\nstate: {}\nvariants:\n{}\nwinner: {}\n",
                cfg.name,
                cfg.state,
                variants,
                cfg.winner.as_deref().unwrap_or("(none)")
            )
        }
        Err(e) => format!("error: {e}\n"),
    }
}

/// Conclude the experiment, pinning "treatment" as the winner.
///
/// After this, every `assign()` call returns "treatment" without emitting
/// a new exposure event.
#[post("/conclude")]
async fn conclude(exps: autumn_web::experiments::Experiments) -> String {
    match exps.service().conclude(EXPERIMENT, "treatment") {
        Ok(()) => format!(
            "experiment '{EXPERIMENT}' concluded — winner: treatment\n\
             All actors will now see the winner variant.\n"
        ),
        Err(e) => format!("error: {e}\n"),
    }
}

#[autumn_web::main]
async fn main() {
    let store = std::sync::Arc::new(InMemoryExperimentStore::new());
    let svc = ExperimentService::new(
        store as std::sync::Arc<dyn autumn_web::experiments::ExperimentStore>,
    );

    // Declare a 50/50 checkout experiment.
    svc.create(
        ExperimentConfig::new(
            EXPERIMENT,
            vec![
                VariantConfig::new("control", 50),
                VariantConfig::new("treatment", 50),
            ],
        )
        .description("Two-variant checkout flow: classic (control) vs. redesign (treatment)"),
    )
    .expect("failed to create experiment");

    // Start the experiment — assignments and exposures are now active.
    svc.start(EXPERIMENT).expect("failed to start experiment");

    // Override qa:alice to "treatment" for manual QA testing.
    svc.set_override(EXPERIMENT, "qa:alice", "treatment")
        .expect("failed to set override");

    println!("✓ Experiment '{EXPERIMENT}' started with 50/50 variants.");
    println!("  Visit http://localhost:3000/checkout/user:1");
    println!("  QA actor 'qa:alice' is overridden to 'treatment'.");
    println!("  Scrape http://localhost:3000/actuator/prometheus for unified metrics.");

    // Shared atomic counters updated by the checkout handler.
    let counters = AssignmentCounters {
        control: Arc::new(AtomicU64::new(0)),
        treatment: Arc::new(AtomicU64::new(0)),
    };

    // Register the experiment metrics source so variant assignment counts
    // appear in /actuator/prometheus alongside autumn_http_* families —
    // no second port or exporter needed.
    let metrics_source = Arc::new(ExperimentMetrics {
        control_assignments: counters.control.clone(),
        treatment_assignments: counters.treatment.clone(),
    });

    autumn_web::app()
        .with_experiment_store_and_sink(
            InMemoryExperimentStore::new(),
            std::sync::Arc::new(autumn_web::experiments::TracingExposureSink),
        )
        .state_initializer(move |state| {
            state.insert_extension(svc.clone());
            state.insert_extension(counters.clone());
        })
        .metrics_source("experiments", metrics_source)
        .routes(routes![checkout, status, conclude])
        .run()
        .await;
}
