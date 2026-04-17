//! Shared application state.
//!
//! This module defines [`AppState`], the core state object passed to all
//! Axum route handlers. It contains framework-managed resources like the
//! database connection pool, metrics collector, and WebSocket channels.
//!
//! Handlers typically don't extract `AppState` directly. Instead, they use
//! specialized extractors like [`Db`](crate::Db) which pull what they need
//! from the state. However, custom extractors can access the state via
//! `axum::extract::State<AppState>`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::actuator;
#[cfg(feature = "ws")]
use crate::channels::Channels;
#[cfg(feature = "db")]
use crate::db::DbState;
use crate::middleware;
use crate::probe;
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
    /// Runtime-managed typed extensions installed by integrations after the app
    /// state has been constructed.
    pub(crate) extensions: Arc<Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,

    /// Database connection pool, or `None` when no `database.url` is configured.
    #[cfg(feature = "db")]
    pub(crate) pool:
        Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,

    /// Active profile name (e.g., "dev", "prod", "staging").
    pub(crate) profile: Option<String>,

    /// When the application started. Used for uptime calculation.
    pub(crate) started_at: std::time::Instant,

    /// Whether the health endpoint should include detailed info.
    pub(crate) health_detailed: bool,

    /// Probe lifecycle state for liveness, readiness, and startup endpoints.
    pub(crate) probes: probe::ProbeState,

    /// In-memory metrics collector for the `/actuator/metrics` endpoint.
    pub(crate) metrics: middleware::MetricsCollector,

    /// Runtime log level state for the `/actuator/loggers` endpoint.
    pub(crate) log_levels: actuator::LogLevels,

    /// Scheduled task registry for the `/actuator/tasks` endpoint.
    pub(crate) task_registry: actuator::TaskRegistry,

    /// Resolved config properties with source tracking for `/actuator/configprops`.
    pub(crate) config_props: actuator::ConfigProperties,

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
    /// Install or replace a typed runtime extension.
    ///
    /// Integrations use this to publish typed runtime resources, such as
    /// background-worker handles or dedicated storage pools, after startup.
    ///
    /// # Panics
    ///
    /// Panics if the internal extension map mutex is poisoned.
    pub fn insert_extension<T>(&self, value: T)
    where
        T: Any + Send + Sync + 'static,
    {
        self.extensions
            .lock()
            .expect("app state extension lock poisoned")
            .insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// Borrow a typed runtime extension if it has been installed.
    ///
    /// The returned [`Arc`] is cloned out of the internal registry so callers
    /// do not hold the state mutex while using the value.
    ///
    /// # Panics
    ///
    /// Panics if the internal extension map mutex is poisoned.
    #[must_use]
    pub fn extension<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        self.extensions
            .lock()
            .expect("app state extension lock poisoned")
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|value| Arc::downcast::<T>(value).ok())
    }

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
    pub const fn log_levels(&self) -> &actuator::LogLevels {
        &self.log_levels
    }

    /// Returns the task registry.
    #[must_use]
    pub const fn task_registry(&self) -> &actuator::TaskRegistry {
        &self.task_registry
    }

    /// Returns the config properties.
    #[must_use]
    pub const fn config_props(&self) -> &actuator::ConfigProperties {
        &self.config_props
    }

    /// Returns the shared probe lifecycle state.
    #[must_use]
    pub const fn probes(&self) -> &probe::ProbeState {
        &self.probes
    }

    /// Mark startup as complete so readiness can become healthy.
    pub fn mark_startup_complete(&self) {
        self.probes.mark_startup_complete();
    }

    /// Mark the application as draining so readiness flips unhealthy.
    pub fn begin_shutdown(&self) {
        self.probes.begin_shutdown();
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

    /// Install a typed runtime extension while building test or ad-hoc state.
    #[must_use]
    pub fn with_extension<T>(self, value: T) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        self.insert_extension(value);
        self
    }

    /// Sets the active profile.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Set the startup probe completion flag.
    #[doc(hidden)]
    #[must_use]
    pub fn with_startup_complete(self, startup_complete: bool) -> Self {
        self.probes.set_startup_complete(startup_complete);
        self
    }

    /// Set the readiness draining flag.
    #[doc(hidden)]
    #[must_use]
    pub fn with_draining(self, draining: bool) -> Self {
        self.probes.set_draining(draining);
        self
    }

    /// Returns the active profile name, or `"default"` if none is set.
    #[must_use]
    pub fn profile(&self) -> &str {
        self.profile.as_deref().unwrap_or("default")
    }

    /// Returns how long the application has been running.
    #[must_use]
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

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
        self.begin_shutdown();
        self.shutdown.cancel();
    }

    /// Update startup completion in tests after the router is already built.
    #[doc(hidden)]
    pub fn set_startup_complete_for_test(&self, startup_complete: bool) {
        self.probes.set_startup_complete(startup_complete);
    }

    /// Update draining state in tests after the router is already built.
    #[doc(hidden)]
    pub fn set_draining_for_test(&self, draining: bool) {
        self.probes.set_draining(draining);
    }

    /// Compatibility helper for tests that model shutdown as readiness drain.
    #[doc(hidden)]
    pub fn begin_shutdown_for_test(&self) {
        self.set_draining_for_test(true);
    }

    /// Create a minimal detached `AppState` without an HTTP server.
    ///
    /// This is useful for background runtimes or helper processes that still
    /// need framework-managed resources such as typed extensions, metrics, or
    /// WebSocket channel registries.
    #[must_use]
    pub fn detached() -> Self {
        Self {
            extensions: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: probe::ProbeState::ready_for_test(),
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        }
    }

    /// Create an `AppState` suitable for testing, with sensible defaults
    /// for all fields. Database pool is `None`.
    #[allow(dead_code)]
    #[must_use]
    pub fn for_test() -> Self {
        Self::detached()
    }
}

#[cfg(feature = "db")]
impl DbState for AppState {
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
    {
        self.pool.as_ref()
    }
}

impl crate::probe::ProvideProbeState for AppState {
    fn probes(&self) -> &crate::probe::ProbeState {
        &self.probes
    }

    fn health_detailed(&self) -> bool {
        self.health_detailed
    }

    fn profile(&self) -> &str {
        self.profile()
    }

    fn uptime_display(&self) -> String {
        self.uptime_display()
    }

    #[cfg(feature = "db")]
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
    {
        self.pool.as_ref()
    }
}

impl crate::actuator::ProvideActuatorState for AppState {
    fn metrics(&self) -> &crate::middleware::MetricsCollector {
        &self.metrics
    }

    fn log_levels(&self) -> &crate::actuator::LogLevels {
        &self.log_levels
    }

    fn task_registry(&self) -> &crate::actuator::TaskRegistry {
        &self.task_registry
    }

    fn config_props(&self) -> &crate::actuator::ConfigProperties {
        &self.config_props
    }

    fn profile(&self) -> &str {
        self.profile()
    }

    fn uptime_display(&self) -> String {
        self.uptime_display()
    }

    #[cfg(feature = "ws")]
    fn channels(&self) -> &crate::channels::Channels {
        &self.channels
    }

    #[cfg(feature = "ws")]
    fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown_token()
    }

    #[cfg(feature = "db")]
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
    {
        self.pool.as_ref()
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
        s.field(
            "extensions",
            &self
                .extensions
                .lock()
                .map_or(0, |extensions| extensions.len())
                ,
        );
        s.field("profile", &self.profile)
            .field("started_at", &self.started_at)
            .field("health_detailed", &self.health_detailed)
            .field("probes", &self.probes)
            .field("metrics", &"MetricsCollector")
            .field("log_levels", &"LogLevels")
            .field("task_registry", &"TaskRegistry")
            .finish_non_exhaustive()
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

    #[test]
    fn detached_state_starts_without_profile() {
        let state = AppState::detached();

        assert_eq!(state.profile(), "default");
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
        let _missing = state.extension::<String>();
    }

    #[test]
    fn app_state_runtime_extensions_round_trip() {
        let state = AppState::for_test();
        state.insert_extension(String::from("haunted"));

        let stored = state
            .extension::<String>()
            .expect("runtime extension should be installed");

        assert_eq!(stored.as_str(), "haunted");
    }
}
