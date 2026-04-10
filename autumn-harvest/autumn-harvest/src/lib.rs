//! Durable workflow orchestration engine for the Autumn web framework.

#[doc(hidden)]
pub mod builder;
#[doc(hidden)]
pub mod cache;
#[doc(hidden)]
pub mod context;
#[doc(hidden)]
pub mod dag;
#[doc(hidden)]
pub mod error;
#[doc(hidden)]
pub mod event;
#[doc(hidden)]
pub mod executor;
#[doc(hidden)]
pub mod info;
#[doc(hidden)]
pub mod policy;
#[doc(hidden)]
pub mod pool;
pub mod prelude;
#[doc(hidden)]
pub mod query;
#[doc(hidden)]
pub mod replay;
#[doc(hidden)]
pub mod types;

#[doc(hidden)]
#[cfg(feature = "db")]
pub mod dlq;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod heartbeat;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod models;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod notify;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod queue;
#[doc(hidden)]
#[cfg(feature = "db")]
#[allow(clippy::wildcard_imports)]
pub mod schema;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod signal;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod store;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod timeout;
#[doc(hidden)]
#[cfg(feature = "db")]
pub mod worker;

pub use builder::{HarvestBuilder, WorkerConfig};
pub use cache::{CachedWorkflowState, WorkflowCache};
pub use context::{ActivityContext, WorkflowCommand, WorkflowContext};
pub use dag::{DagBuildError, DagBuilder, DagDefinition, DagTask, DagTaskRef};
pub use error::{HarvestError, HarvestResult, TimeoutType};
pub use event::WorkflowEvent;
pub use executor::{WorkflowOutcome, run_workflow};
pub use info::{ActivityHandlerFn, ActivityInfo, DagInfo, WorkflowHandlerFn, WorkflowInfo};
pub use policy::{RetryPolicy, Schedule, TaskStatus, TriggerRule};
pub use pool::{HarvestPoolConfig, compute_pool_sizes};
pub use query::QueryRegistry;
pub use replay::{HistoryMatch, HistoryMatcher};
pub use types::{ActivityExecId, ExecutionId, TimerId, WorkerId, WorkflowId};

#[cfg(feature = "db")]
pub use store::EventHistory;

// Allow macro-generated code to use ::autumn_harvest::serde_json
pub use serde_json;

/// Parse a human-readable duration string like `"5m"`, `"30s"`, `"1h"`.
///
/// Used by macro-generated code — not intended for direct use.
#[doc(hidden)]
#[must_use]
pub fn task_duration(s: &str) -> Option<std::time::Duration> {
    autumn_web::task::parse_duration(s)
}
