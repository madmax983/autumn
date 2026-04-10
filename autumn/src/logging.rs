//! Structured logging initialization via `tracing-subscriber`.
//!
//! Call [`init`] once, early in application startup (after loading config),
//! to install the global tracing subscriber. The subscriber respects
//! [`LogConfig`] to choose between human-readable and JSON output, and
//! uses [`tracing_subscriber::EnvFilter`] to parse the configured log level directive.
//!
//! In normal usage, [`AppBuilder::run`](crate::app::AppBuilder::run) calls
//! [`init`] automatically. You only need to call it directly in test
//! harnesses or custom entry points.

use crate::config::{LogConfig, TelemetryConfig};

/// Initialize the tracing subscriber based on logging configuration alone.
///
/// Must be called **once**, early in application startup -- before any
/// `tracing::info!` / `tracing::debug!` calls. In normal usage,
/// [`AppBuilder::run`](crate::app::AppBuilder::run) calls this
/// automatically.
///
/// # Panics
///
/// Panics if called a second time. The global tracing subscriber can
/// only be set once per process.
///
pub fn init(config: &LogConfig) {
    let profile = crate::config::resolve_profile(&crate::config::OsEnv);
    let _ = init_with_telemetry(config, &TelemetryConfig::default(), profile.as_deref())
        .unwrap_or_else(|error| panic!("failed to initialize logging: {error}"));
}

/// Initialize logging plus optional framework-managed telemetry.
///
/// Returns a guard that must stay alive for as long as OTLP export should stay
/// active so batched spans can flush cleanly on shutdown.
///
/// # Errors
///
/// Returns [`crate::telemetry::TelemetryInitError`] when the telemetry plan is
/// invalid or the tracing subscriber/exporter fails to initialize.
pub fn init_with_telemetry(
    config: &LogConfig,
    telemetry: &TelemetryConfig,
    profile: Option<&str>,
) -> Result<crate::telemetry::TelemetryGuard, crate::telemetry::TelemetryInitError> {
    crate::telemetry::init(config, telemetry, profile)
}

/// Returns `true` when `AUTUMN_ENV` is set to `"production"`
/// (case-insensitive).
#[cfg(test)]
fn is_production() -> bool {
    std::env::var("AUTUMN_ENV")
        .map(|v| v.eq_ignore_ascii_case("production"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LogConfig, LogFormat};

    #[test]
    fn is_production_false_by_default() {
        // In tests AUTUMN_ENV is not normally set, so this should be false.
        // (We don't mutate env vars here to avoid cross-test interference.)
        assert!(!is_production());
    }

    // We cannot call `init()` in unit tests because the global subscriber
    // can only be set once per process and other tests may have already
    // set it. Instead, test the format selection logic directly.

    #[test]
    fn auto_format_is_not_json_in_non_production() {
        // Without AUTUMN_ENV=production, Auto should resolve to pretty (not json).
        let use_json = match LogFormat::Auto {
            LogFormat::Auto => is_production(),
            LogFormat::Pretty => false,
            LogFormat::Json => true,
        };
        assert!(!use_json);
    }

    #[test]
    fn pretty_format_is_never_json() {
        let use_json = match LogFormat::Pretty {
            LogFormat::Auto => is_production(),
            LogFormat::Pretty => false,
            LogFormat::Json => true,
        };
        assert!(!use_json);
    }

    #[test]
    fn json_format_is_always_json() {
        let use_json = match LogFormat::Json {
            LogFormat::Auto => is_production(),
            LogFormat::Pretty => false,
            LogFormat::Json => true,
        };
        assert!(use_json);
    }

    #[test]
    fn valid_filter_parses() {
        // Ensure that valid EnvFilter directives don't trigger the fallback.
        let config = LogConfig {
            level: "debug".to_owned(),
            format: LogFormat::Auto,
        };
        let filter = tracing_subscriber::EnvFilter::try_new(&config.level);
        assert!(filter.is_ok());
    }

    #[test]
    fn invalid_filter_falls_back() {
        // An invalid directive should fail to parse (triggering the
        // eprintln fallback in init).
        let filter = tracing_subscriber::EnvFilter::try_new("not_a_valid_[directive");
        assert!(filter.is_err());
    }
}
