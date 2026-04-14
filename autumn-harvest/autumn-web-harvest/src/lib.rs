//! Autumn adapter crate for autumn-harvest.

pub mod api;
pub mod config;
pub mod ext;
pub mod outbox;
pub mod prelude;
pub mod runner;
pub mod state;

pub use api::{HarvestApiRuntime, HarvestApiState, harvest_api_router};
pub use config::{HarvestDatabaseConfig, HarvestMode, HarvestOutboxConfig, HarvestRuntimeConfig};
pub use ext::HarvestExt;
pub use outbox::{
    WorkflowStartRequest, drain_workflow_start_outbox_once, enqueue_workflow_start_outbox,
    flush_workflow_start_outbox,
};
pub use runner::{HarvestRunner, HarvestRunnerResources};
pub use state::{AppDbPool, HarvestDbPool};
