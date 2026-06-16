use std::sync::Arc;

use autumn_web::experiments::{
    ExperimentConfig, ExperimentService, InMemoryExperimentStore, TracingExposureSink,
    VariantConfig,
};

/// Provision the experiment service and seed the feed_layout A/B experiment.
///
/// **feed_layout** — 50/50 split between:
/// - `"compact"`: dense list with no card shadows, more posts above the fold.
/// - `"card"`: current card layout with borders and vote controls.
///
/// Assignments are sticky per actor (session user_id or stable anonymous key).
/// When a winner is clear, conclude the experiment via the service and
/// every actor will see the winning variant without a restart.
///
/// In production, swap `InMemoryExperimentStore` for `PgExperimentStore` so
/// assignments survive restarts and you can toggle experiments from the DB.
pub fn setup() -> ExperimentService {
    let store = Arc::new(InMemoryExperimentStore::new());
    let svc = ExperimentService::new(Arc::clone(&store) as Arc<_>)
        .with_exposure_sink(Arc::new(TracingExposureSink));

    svc.create(
        ExperimentConfig::new(
            "feed_layout",
            vec![
                VariantConfig::new("compact", 50),
                VariantConfig::new("card", 50),
            ],
        )
        .description("50/50 split: compact list (control) vs. card layout (treatment)"),
    )
    .expect("failed to create feed_layout experiment");

    svc.start("feed_layout")
        .expect("failed to start feed_layout experiment");

    svc
}
