//! Structured logging initialization via `tracing-subscriber`.
//!
//! Call [`init`] once, early in application startup (after loading config),
//! to install the global tracing subscriber.  The subscriber respects
//! [`LogConfig`] to choose between human-readable and JSON output, and
//! uses [`EnvFilter`] to parse the configured log level directive.

use crate::config::{LogConfig, LogFormat};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize the tracing subscriber based on configuration.
///
/// Must be called **once**, early in application startup — before any
/// `tracing::info!` / `tracing::debug!` calls.  Calling it a second
/// time will panic (the global subscriber can only be set once).
///
/// # Format selection
///
/// | `LogFormat` | Behaviour |
/// |-------------|-----------|
/// | `Auto`      | JSON when `AUTUMN_ENV=production`, pretty otherwise |
/// | `Pretty`    | Always human-readable |
/// | `Json`      | Always structured JSON |
///
/// # Filter fallback
///
/// If the configured `level` string is not a valid `EnvFilter` directive,
/// a warning is printed to stderr and the filter falls back to `"info"`.
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

    #[test]
    fn is_production_false_by_default() {
        // In tests AUTUMN_ENV is not normally set, so this should be false.
        // (We don't mutate env vars here to avoid cross-test interference.)
        assert!(!is_production());
    }
}
