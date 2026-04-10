//! Structured logging initialization via `tracing-subscriber`.
//!
//! Call [`init`] once, early in application startup (after loading config),
//! to install the global tracing subscriber. The subscriber respects
//! [`LogConfig`] to choose between human-readable and JSON output, and
//! uses [`EnvFilter`] to parse the configured log level directive.
//!
//! In normal usage, [`AppBuilder::run`](crate::app::AppBuilder::run) calls
//! [`init`] automatically. You only need to call it directly in test
//! harnesses or custom entry points.

use crate::config::{LogConfig, LogFormat};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize the tracing subscriber based on configuration.
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
/// # Format selection
///
/// | [`LogFormat`] | Behaviour |
/// |-------------|-----------|
/// | `Auto` | JSON when `AUTUMN_ENV=production`, pretty otherwise |
/// | `Pretty` | Always human-readable, colorized |
/// | `Json` | Always structured JSON |
///
/// # Filter fallback
///
/// If the configured `level` string is not a valid `EnvFilter` directive,
/// a warning is printed to stderr and the filter falls back to `"info"`.
///
/// # Parameters
///
/// - `config`: A reference to the [`LogConfig`] detailing the log level
///   and format.
pub fn init(config: &LogConfig) {
    let filter = EnvFilter::try_new(&config.level).unwrap_or_else(|e| {
        eprintln!(
            "Warning: invalid log filter {:?}: {e}, falling back to \"info\"",
            config.level
        );
        EnvFilter::new("info")
    });

    let use_json = match config.format {
        LogFormat::Auto => is_production(),
        LogFormat::Pretty => false,
        LogFormat::Json => true,
    };

    if use_json {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty())
            .init();
    }
}

/// Returns `true` when `AUTUMN_ENV` is set to `"production"`
/// (case-insensitive).
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
