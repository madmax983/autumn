use autumn_harvest::HarvestBuilder;
use autumn_harvest::prelude::WorkerConfig;
use autumn_web::AppBuilder;
use autumn_web_harvest::HarvestExt;

use crate::workflows;

pub const HARVEST_API_PATH: &str = "/api/harvest";

#[must_use]
pub fn configured_worker_config() -> WorkerConfig {
    WorkerConfig {
        max_concurrent_workflows: 4,
        max_concurrent_activities: 8,
        ..WorkerConfig::default()
    }
}

#[must_use]
pub fn harvest_builder() -> HarvestBuilder {
    HarvestBuilder::new()
        .workflows(workflows::registered_workflows())
        .activities(workflows::registered_activities())
        .worker(configured_worker_config())
}

#[must_use]
pub fn configure_embedded_harvest(builder: AppBuilder) -> AppBuilder {
    let worker_config = configured_worker_config();

    builder
        .workflows(workflows::registered_workflows())
        .activities(workflows::registered_activities())
        .worker(worker_config)
        .harvest_api(HARVEST_API_PATH)
}

#[cfg(test)]
mod tests {
    use autumn_harvest::HarvestBuilder;

    use super::{HARVEST_API_PATH, configured_worker_config, harvest_builder};

    #[test]
    fn shared_harvest_builder_reuses_registered_workflows_and_activities() {
        let builder: HarvestBuilder = harvest_builder();
        let built = builder.build();

        assert_eq!(built.workflow_count(), 2);
        assert_eq!(built.activity_count(), 3);
        assert_eq!(built.worker_config().max_concurrent_workflows, 4);
        assert_eq!(built.worker_config().max_concurrent_activities, 8);
    }

    #[test]
    fn configured_worker_config_matches_embedded_web_defaults() {
        let config = configured_worker_config();

        assert_eq!(config.max_concurrent_workflows, 4);
        assert_eq!(config.max_concurrent_activities, 8);
        assert_eq!(config.queues, vec!["default".to_string()]);
    }

    #[test]
    fn harvest_api_path_stays_stable_for_runner_and_web_docs() {
        assert_eq!(HARVEST_API_PATH, "/api/harvest");
    }
}
