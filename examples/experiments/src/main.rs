//! A/B experiment example: two-variant checkout flow with exposure events.
//!
//! Demonstrates:
//! - Declaring a named experiment with two 50/50 variants
//! - Deterministic, sticky actor assignment
//! - Exposure events logged to stdout via the default tracing sink
//! - Staff/QA override pinning a specific actor to a variant
//! - Concluding an experiment with a winner
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
//! # Conclude the experiment (returns winner for everyone)
//! curl -X POST http://localhost:3000/conclude
//! curl http://localhost:3000/checkout/user:99
//! ```

use autumn_web::experiments::{
    ExperimentConfig, ExperimentService, InMemoryExperimentStore, VariantConfig,
};
use autumn_web::prelude::*;

const EXPERIMENT: &str = "checkout_v2";

/// Assign a variant to the given actor and return it as plain text.
///
/// Exposure events are emitted automatically to tracing (stdout in this example).
#[get("/checkout/{actor}")]
async fn checkout(Path(actor): Path<String>, exps: autumn_web::experiments::Experiments) -> String {
    match exps.service().assign(EXPERIMENT, &actor) {
        Ok(variant) => format!(
            "actor={actor} experiment={EXPERIMENT} variant={variant}\n\
             (exposure event logged to stdout)\n"
        ),
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

    autumn_web::app()
        .with_experiment_store_and_sink(
            InMemoryExperimentStore::new(),
            std::sync::Arc::new(autumn_web::experiments::TracingExposureSink),
        )
        .state_initializer(move |state| {
            state.insert_extension(svc.clone());
        })
        .routes(routes![checkout, status, conclude])
        .run()
        .await;
}
