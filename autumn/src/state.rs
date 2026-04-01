use crate::actuator;
#[cfg(feature = "ws")]
use crate::channels::Channels;
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
/// let state = AppState {
///     pool: None,
///     profile: Some("dev".into()),
///     started_at: std::time::Instant::now(),
///     health_detailed: true,
///     metrics: autumn_web::middleware::MetricsCollector::new(),
///     log_levels: autumn_web::actuator::LogLevels::new("info"),
///     task_registry: autumn_web::actuator::TaskRegistry::new(),
///     config_props: Default::default(),
///     #[cfg(feature = "ws")]
///     channels: autumn_web::channels::Channels::new(32),
///     #[cfg(feature = "ws")]
///     shutdown: tokio_util::sync::CancellationToken::new(),
/// };
/// ```
#[derive(Clone)]
pub struct AppState {
    /// Shared application state passed to all route handlers.
    /// Database connection pool, or `None` when no `database.url` is configured.
    #[cfg(feature = "db")]
    pub pool:
        Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,

    /// Shared application state passed to all route handlers.
    /// Active profile name (e.g., "dev", "prod", "staging").
    pub profile: Option<String>,

    /// Shared application state passed to all route handlers.
    /// When the application started. Used for uptime calculation.
    pub started_at: std::time::Instant,

    /// Shared application state passed to all route handlers.
    /// Whether the health endpoint should include detailed info.
    pub health_detailed: bool,

    /// Shared application state passed to all route handlers.
    /// In-memory metrics collector for the `/actuator/metrics` endpoint.
    pub metrics: middleware::MetricsCollector,

    /// Shared application state passed to all route handlers.
    /// Runtime log level state for the `/actuator/loggers` endpoint.
    pub log_levels: actuator::LogLevels,

    /// Shared application state passed to all route handlers.
    /// Scheduled task registry for the `/actuator/tasks` endpoint.
    pub task_registry: actuator::TaskRegistry,

    /// Shared application state passed to all route handlers.
    /// Resolved config properties with source tracking for `/actuator/configprops`.
    pub config_props: actuator::ConfigProperties,

    /// Named broadcast channel registry for real-time messaging.
    ///
    /// Available when the `ws` feature is enabled. Use
    /// [`channels()`](Self::channels) for convenient access.
    #[cfg(feature = "ws")]
    pub channels: Channels,

    /// Cancellation token signalled during graceful shutdown.
    ///
    /// WebSocket handlers receive a child token so they can clean up
    /// when the server is stopping.
    #[cfg(feature = "ws")]
    pub shutdown: CancellationToken,
}

impl AppState {
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

    /// Create an `AppState` suitable for testing, with sensible defaults
    /// for all fields. Database pool is `None`.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_test() -> Self {
        Self {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "db")]
    use crate::config;
    #[cfg(feature = "db")]
    use crate::db;

    #[test]
    fn app_state_debug_without_pool() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: Some("dev".into()),
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
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
        let state = AppState {
            pool: Some(pool),
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
        let debug = format!("{state:?}");
        assert!(debug.contains("Pool(max=5)"));
    }

    fn require_clone<T: Clone>(t: &T) -> T {
        t.clone()
    }

    #[test]
    fn app_state_is_clone() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
        let _cloned = require_clone(&state);
    }

    #[test]
    fn app_state_profile_accessor() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: Some("staging".into()),
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
        assert_eq!(state.profile(), "staging");
    }

    #[test]
    fn app_state_profile_default() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
        assert_eq!(state.profile(), "default");
    }

    #[test]
    fn app_state_uptime_display() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: CancellationToken::new(),
        };
        let display = state.uptime_display();
        assert!(
            display.contains('s'),
            "uptime should contain 's': {display}"
        );
    }
}
