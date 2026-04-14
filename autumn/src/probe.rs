//! Liveness, readiness, and startup probes.
//!
//! Autumn exposes explicit cloud-native probe contracts:
//! - liveness ignores startup and dependency state
//! - readiness reflects startup completion, shutdown draining, and core dependencies
//! - startup stays unavailable until startup hooks complete

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

/// Trait to abstract the state requirements for probe handlers.
pub trait ProvideProbeState {
    fn probes(&self) -> &ProbeState;
    fn health_detailed(&self) -> bool;
    fn profile(&self) -> &str;
    fn uptime_display(&self) -> String;

    #[cfg(feature = "db")]
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>;

    fn mark_startup_complete(&self) {
        self.probes().mark_startup_complete();
    }
}

/// Shared probe lifecycle state stored in `AppState`.
#[derive(Clone, Debug, Default)]
pub struct ProbeState {
    startup_complete: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
}

impl ProbeState {
    /// Create a probe state that starts in pending-startup mode.
    #[must_use]
    pub fn pending_startup() -> Self {
        Self::default()
    }

    /// Alias for pending startup used by application bootstrapping.
    #[must_use]
    pub fn starting() -> Self {
        Self::pending_startup()
    }

    /// Create a probe state that is immediately ready.
    #[must_use]
    pub fn ready_for_test() -> Self {
        let state = Self::pending_startup();
        state.mark_startup_complete();
        state
    }

    /// Mark startup as complete and readiness eligible.
    pub fn mark_startup_complete(&self) {
        self.startup_complete.store(true, Ordering::Relaxed);
    }

    /// Override startup completion for tests.
    pub fn set_startup_complete(&self, complete: bool) {
        self.startup_complete.store(complete, Ordering::Relaxed);
    }

    /// Mark the application as shutting down so readiness flips false.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
    }

    /// Alias for readiness drain used during graceful shutdown.
    pub fn begin_draining(&self) {
        self.begin_shutdown();
    }

    /// Override shutdown-draining state for tests.
    pub fn set_draining(&self, draining: bool) {
        self.shutting_down.store(draining, Ordering::Relaxed);
    }

    /// Returns whether startup completed successfully.
    #[must_use]
    pub fn is_startup_complete(&self) -> bool {
        self.startup_complete.load(Ordering::Relaxed)
    }

    /// Returns whether graceful shutdown has started.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }

    /// Returns whether readiness is currently draining.
    #[must_use]
    pub fn draining(&self) -> bool {
        self.is_shutting_down()
    }
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Live,
    Ready,
    Startup,
}

#[derive(Serialize)]
pub(crate) struct ProbeResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<PoolStatus>,
}

#[derive(Serialize)]
pub(crate) struct PoolStatus {
    size: u64,
    available: u64,
    waiting: u64,
}

fn dependency_readiness<S: ProvideProbeState>(state: &S) -> (bool, Option<PoolStatus>) {
    #[cfg(feature = "db")]
    {
        if let Some(pool) = state.pool() {
            let status = pool.status();
            let available = status.available as u64;
            let size = status.max_size as u64;
            let waiting = status.waiting as u64;

            return (
                available > 0 || waiting == 0,
                Some(PoolStatus {
                    size,
                    available,
                    waiting,
                }),
            );
        }
    }

    (true, None)
}

fn probe_response<S: ProvideProbeState>(
    state: &S,
    kind: ProbeKind,
) -> (StatusCode, Json<ProbeResponse>) {
    let startup_complete = state.probes().is_startup_complete();
    let shutting_down = state.probes().is_shutting_down();
    let (dependencies_ready, pool_status) = dependency_readiness(state);

    let (status_code, status) = match kind {
        ProbeKind::Live => (StatusCode::OK, "ok"),
        ProbeKind::Startup if startup_complete => (StatusCode::OK, "ok"),
        ProbeKind::Startup => (StatusCode::SERVICE_UNAVAILABLE, "starting"),
        ProbeKind::Ready if startup_complete && !shutting_down && dependencies_ready => {
            (StatusCode::OK, "ok")
        }
        ProbeKind::Ready => (StatusCode::SERVICE_UNAVAILABLE, "degraded"),
    };

    let detailed = state.health_detailed();
    let body = ProbeResponse {
        status,
        version: if detailed {
            Some(env!("CARGO_PKG_VERSION"))
        } else {
            None
        },
        profile: if detailed {
            Some(state.profile().to_owned())
        } else {
            None
        },
        uptime: if detailed {
            Some(state.uptime_display())
        } else {
            None
        },
        pool: if detailed { pool_status } else { None },
    };

    (status_code, Json(body))
}

/// `GET /live`
pub async fn live_handler<S: ProvideProbeState + Send + Sync + 'static>(
    State(state): State<S>,
) -> impl IntoResponse {
    probe_response(&state, ProbeKind::Live)
}

/// `GET /ready`
pub async fn ready_handler<S: ProvideProbeState + Send + Sync + 'static>(
    State(state): State<S>,
) -> impl IntoResponse {
    probe_response(&state, ProbeKind::Ready)
}

/// `GET /startup`
pub async fn startup_handler<S: ProvideProbeState + Send + Sync + 'static>(
    State(state): State<S>,
) -> impl IntoResponse {
    probe_response(&state, ProbeKind::Startup)
}

/// Compatibility alias for the legacy `/health` endpoint.
pub(crate) fn readiness_response<S: ProvideProbeState>(
    state: &S,
) -> (StatusCode, Json<ProbeResponse>) {
    probe_response(state, ProbeKind::Ready)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProbeState {
        probes: ProbeState,
        health_detailed: bool,
        profile: String,
    }

    impl ProvideProbeState for TestProbeState {
        fn probes(&self) -> &ProbeState {
            &self.probes
        }

        fn health_detailed(&self) -> bool {
            self.health_detailed
        }

        fn profile(&self) -> &str {
            &self.profile
        }

        fn uptime_display(&self) -> String {
            "test uptime".to_string()
        }

        #[cfg(feature = "db")]
        fn pool(
            &self,
        ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
        {
            None
        }
    }

    impl TestProbeState {
        fn new() -> Self {
            Self {
                probes: ProbeState::pending_startup(),
                health_detailed: true,
                profile: "test".to_string(),
            }
        }
    }

    #[test]
    fn test_live_handler_returns_ok() {
        let state = TestProbeState::new();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Live);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.status, "ok");
    }

    #[tokio::test]
    async fn test_startup_handler_pending() {
        let state = TestProbeState::new();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Startup);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.status, "starting");
    }

    #[tokio::test]
    async fn test_startup_handler_complete() {
        let state = TestProbeState::new();
        state.mark_startup_complete();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Startup);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.status, "ok");
    }

    #[tokio::test]
    async fn test_ready_handler_pending_startup() {
        let state = TestProbeState::new();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Ready);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.status, "degraded");
    }

    #[tokio::test]
    async fn test_ready_handler_complete_startup() {
        let state = TestProbeState::new();
        state.mark_startup_complete();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Ready);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.status, "ok");
    }

    #[tokio::test]
    async fn test_ready_handler_shutting_down() {
        let state = TestProbeState::new();
        state.mark_startup_complete();
        state.probes().begin_shutdown();
        let (status, Json(response)) = probe_response(&state, ProbeKind::Ready);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.status, "degraded");
    }

    #[tokio::test]
    async fn test_probe_state_set_draining() {
        let state = ProbeState::starting();
        assert!(!state.draining());
        state.set_draining(true);
        assert!(state.draining());
    }

    #[tokio::test]
    async fn test_probe_state_set_startup_complete() {
        let state = ProbeState::starting();
        assert!(!state.is_startup_complete());
        state.set_startup_complete(true);
        assert!(state.is_startup_complete());
    }

    #[tokio::test]
    async fn test_ready_for_test() {
        let state = ProbeState::ready_for_test();
        assert!(state.is_startup_complete());
    }

    #[tokio::test]
    async fn test_health_detailed_false() {
        let mut state = TestProbeState::new();
        state.health_detailed = false;

        let (_, Json(response)) = probe_response(&state, ProbeKind::Live);
        assert!(response.version.is_none());
        assert!(response.profile.is_none());
        assert!(response.uptime.is_none());
        assert!(response.pool.is_none());
    }

    #[tokio::test]
    async fn test_begin_draining() {
        let state = ProbeState::ready_for_test();
        assert!(!state.draining());
        state.begin_draining();
        assert!(state.draining());
    }
}
