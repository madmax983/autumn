#[cfg(feature = "ws")]
use crate::channels::Channels;
use crate::diagnostics;
use crate::middleware;
#[cfg(feature = "ws")]
use tokio_util::sync::CancellationToken;

/// Shared application state passed to all route handlers.
///
/// Holds framework-managed resources such as the database connection pool.
/// Axum requires handler state to be [`Clone`], so internal resources use
/// `Arc` or are already cheaply cloneable (`deadpool::Pool` is `Arc`-wrapped
/// internally).
///
/// This struct is normally constructed by [`crate::app::AppBuilder::run`] and
/// should not need to be created manually. It is public so that custom
/// Axum extractors can access framework resources via
/// `State<AppState>`.
///
/// # Examples
///
/// ```rust
/// use autumn_web::AppState;
///
/// // State without a database (e.g., for testing)
/// let state = AppState::for_test().with_profile("dev");
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct AppState {
    /// Shared application state passed to all route handlers.
    /// Database connection pool, or `None` when no `database.url` is configured.
    #[cfg(feature = "db")]
    pub(crate) pool:
        Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,

    /// Shared application state passed to all route handlers.
    /// Active profile name (e.g., "dev", "prod", "staging").
    pub(crate) profile: Option<String>,

    /// Shared application state passed to all route handlers.
    /// When the application started. Used for uptime calculation.
    pub(crate) started_at: std::time::Instant,

    /// Shared application state passed to all route handlers.
    /// Whether the health endpoint should include detailed info.
    pub(crate) health_detailed: bool,

    /// Shared application state passed to all route handlers.
    /// In-memory metrics collector for the `/actuator/metrics` endpoint.
    pub(crate) metrics: middleware::MetricsCollector,

    /// Shared application state passed to all route handlers.
    /// Runtime log level state for the `/actuator/loggers` endpoint.
    pub(crate) log_levels: diagnostics::LogLevels,

    /// Shared application state passed to all route handlers.
    /// Scheduled task registry for the `/actuator/tasks` endpoint.
    pub(crate) task_registry: diagnostics::TaskRegistry,

    /// Shared application state passed to all route handlers.
    /// Resolved config properties with source tracking for `/actuator/configprops`.
    pub(crate) config_props: diagnostics::ConfigProperties,

    /// Named broadcast channel registry for real-time messaging.
    ///
    /// Available when the `ws` feature is enabled. Use
    /// [`channels()`](Self::channels) for convenient access.
    #[cfg(feature = "ws")]
    pub(crate) channels: Channels,

    /// Cancellation token signalled during graceful shutdown.
    ///
    /// WebSocket handlers receive a child token so they can clean up
    /// when the server is stopping.
    #[cfg(feature = "ws")]
    pub(crate) shutdown: CancellationToken,
}

impl AppState {
    /// Returns the database connection pool.
    #[cfg(feature = "db")]
    #[must_use]
    pub const fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
    {
        self.pool.as_ref()
    }

    /// Returns the metrics collector.
    #[must_use]
    pub const fn metrics(&self) -> &middleware::MetricsCollector {
        &self.metrics
    }

    /// Returns the log levels configuration.
    #[must_use]
    pub const fn log_levels(&self) -> &diagnostics::LogLevels {
        &self.log_levels
    }

    /// Returns the task registry.
    #[must_use]
    pub const fn task_registry(&self) -> &diagnostics::TaskRegistry {
        &self.task_registry
    }

    /// Returns the config properties.
    #[must_use]
    pub const fn config_props(&self) -> &diagnostics::ConfigProperties {
        &self.config_props
    }

    /// Sets the database pool.
    #[cfg(feature = "db")]
    #[must_use]
    pub fn with_pool(
        mut self,
        pool: diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    ) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Sets the active profile.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Shared application state passed to all route handlers.
    /// Returns the active profile name, or `"default"` if none is set.
    #[must_use]
    pub fn profile(&self) -> &str {
        self.profile.as_deref().unwrap_or("default")
    }

    /// Shared application state passed to all route handlers.
    /// Returns how long the application has been running.
    #[must_use]
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Shared application state passed to all route handlers.
    /// Format uptime as a human-readable string (e.g., "2h 15m").
    #[must_use]
    pub fn uptime_display(&self) -> String {
        let secs = self.started_at.elapsed().as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{hours}h {mins}m")
        }
    }

    /// Returns a reference to the broadcast channel registry.
    ///
    /// Shorthand for accessing `self.channels` directly.
    #[cfg(feature = "ws")]
    #[must_use]
    pub const fn channels(&self) -> &Channels {
        &self.channels
    }

    /// Returns a child cancellation token for the server shutdown signal.
    ///
    /// WebSocket handlers should select on this to clean up when the
    /// server is shutting down.
    #[cfg(feature = "ws")]
    #[must_use]
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.child_token()
    }

    /// Helper for integration tests to simulate a server shutdown.
    #[cfg(feature = "ws")]
    #[doc(hidden)]
    pub fn trigger_shutdown_for_test(&self) {
        self.shutdown.cancel();
    }

    /// Create an `AppState` suitable for testing, with sensible defaults
    /// for all fields. Database pool is `None`.
    #[allow(dead_code)]
    #[must_use]
    pub fn for_test() -> Self {
        Self {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: diagnostics::LogLevels::new("info"),
            task_registry: diagnostics::TaskRegistry::new(),
            config_props: diagnostics::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("AppState");
        #[cfg(feature = "db")]
        s.field(
            "pool",
            &self
                .pool
                .as_ref()
                .map(|p| format!("Pool(max={})", p.status().max_size)),
        );
        s.field("profile", &self.profile)
            .field("started_at", &self.started_at)
            .field("health_detailed", &self.health_detailed)
            .field("metrics", &"MetricsCollector")
            .field("log_levels", &"LogLevels")
            .field("task_registry", &"TaskRegistry")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "db")]
impl crate::db::DbState for AppState {
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
    {
        self.pool.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "db")]
    use crate::config;
    #[cfg(feature = "db")]
    use crate::db;

    #[test]
    fn app_state_debug_without_pool() {
        let state = AppState::for_test().with_profile("dev");
        let debug = format!("{state:?}");
        assert!(debug.contains("AppState"));
        assert!(debug.contains("dev"));
    }

    #[cfg(feature = "db")]
    #[test]
    fn app_state_debug_with_pool() {
        let config = config::DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 5,
            ..Default::default()
        };
        let pool = db::create_pool(&config).unwrap().unwrap();
        let state = AppState::for_test().with_pool(pool);
        let debug = format!("{state:?}");
        assert!(debug.contains("Pool(max=5)"));
    }

    fn require_clone<T: Clone>(t: &T) -> T {
        t.clone()
    }

    #[test]
    fn app_state_is_clone() {
        let state = AppState::for_test();
        let _cloned = require_clone(&state);
    }

    #[test]
    fn app_state_profile_accessor() {
        let state = AppState::for_test().with_profile("staging");
        assert_eq!(state.profile(), "staging");
    }

    #[test]
    fn app_state_profile_default() {
        let state = AppState::for_test();
        assert_eq!(state.profile(), "default");
    }

    #[test]
    fn app_state_uptime_display() {
        let state = AppState::for_test();
        let display = state.uptime_display();
        assert!(
            display.contains('s'),
            "uptime should contain 's': {display}"
        );
    }

    #[test]
    fn app_state_accessors() {
        let state = AppState::for_test();

        // Exercise the new getters to ensure they compile and return the expected types
        let _metrics = state.metrics();
        let _log_levels = state.log_levels();
        let _task_registry = state.task_registry();
        let _config_props = state.config_props();

        #[cfg(feature = "db")]
        {
            let _pool = state.pool();
        }
    }
}
