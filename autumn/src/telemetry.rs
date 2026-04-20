//! Telemetry runtime planning and subscriber initialization.
//!
//! The pure planning surface ([`TelemetryRuntime::from_config`]) is testable
//! without touching global tracing state. Actual OTLP exporter installation is
//! feature-gated and happens via [`init`].

use crate::config::{LogConfig, LogFormat, TelemetryConfig, TelemetryProtocol};
use http::Uri;
use thiserror::Error;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature = "telemetry-otlp")]
use opentelemetry::{KeyValue, trace::TracerProvider as _};
#[cfg(feature = "telemetry-otlp")]
use opentelemetry_otlp::WithExportConfig as _;
#[cfg(feature = "telemetry-otlp")]
use opentelemetry_sdk::{
    Resource, propagation::TraceContextPropagator, trace::SdkTracerProvider,
};

/// Concrete log formatting chosen for the running process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedLogFormat {
    /// Human-readable developer logs.
    Pretty,
    /// Structured JSON logs for aggregation pipelines.
    Json,
}

/// Resolved telemetry runtime shape derived from config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryRuntime {
    /// Concrete log format that should be installed.
    pub log_format: ResolvedLogFormat,
    /// Trace exporter plan for the process.
    pub trace_export: TraceExport,
    /// Optional warning describing why tracing fell back to logging-only mode.
    pub warning: Option<String>,
}

/// Trace export plan for the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceExport {
    /// Logging only.
    Disabled,
    /// OTLP trace export.
    Otlp(OtlpTraceRuntime),
}

/// OTLP exporter runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtlpTraceRuntime {
    /// Collector endpoint.
    pub endpoint: String,
    /// Wire protocol.
    pub protocol: TelemetryProtocol,
    /// Resource attributes describing this service.
    pub resource: TelemetryResource,
}

/// Service metadata attached to emitted traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryResource {
    /// Logical service name.
    pub service_name: String,
    /// Optional service namespace.
    pub service_namespace: Option<String>,
    /// Service version string.
    pub service_version: String,
    /// Deployment environment label.
    pub environment: String,
}

/// Errors that can occur while planning or initializing telemetry.
#[derive(Debug, Error)]
pub enum TelemetryInitError {
    /// Telemetry was enabled without an OTLP endpoint.
    #[error("telemetry is enabled but no OTLP endpoint was configured")]
    MissingEndpoint,
    /// `service_name` was empty.
    #[error("telemetry service_name must not be empty")]
    EmptyServiceName,
    /// OTLP endpoint was not a valid absolute URI.
    #[error("invalid OTLP endpoint {endpoint:?}: {reason}")]
    InvalidEndpoint {
        /// The endpoint that was provided.
        endpoint: String,
        /// The reason the endpoint is considered invalid.
        reason: String,
    },
    /// The exporter feature is not compiled in.
    #[error("telemetry-otlp cargo feature is not enabled")]
    #[allow(dead_code)]
    FeatureDisabled,
    /// Exporter initialization failed.
    #[error("failed to initialize OTLP exporter: {0}")]
    #[allow(dead_code)]
    #[allow(dead_code)]
    #[allow(dead_code)]
    #[allow(dead_code)]
    ExporterInit(String),
    /// Global subscriber installation failed.
    #[error("failed to initialize tracing subscriber: {0}")]
    SubscriberInit(String),
}

/// RAII handle that flushes the OTLP tracer provider on drop.
#[must_use]
#[derive(Debug)]
pub struct TelemetryGuard {
    #[cfg(feature = "telemetry-otlp")]
    provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    /// Construct a no-op telemetry guard.
    ///
    /// Useful for [`TelemetryProvider`] implementations that decide at runtime
    /// not to register a global tracing subscriber — e.g. a Datadog provider
    /// reading a feature flag, or any custom impl that wants to opt out of
    /// telemetry without panicking.
    pub const fn disabled() -> Self {
        Self {
            #[cfg(feature = "telemetry-otlp")]
            provider: None,
        }
    }

    #[cfg(feature = "telemetry-otlp")]
    const fn with_provider(provider: SdkTracerProvider) -> Self {
        Self {
            provider: Some(provider),
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "telemetry-otlp")]
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

impl TelemetryRuntime {
    /// Resolve telemetry runtime behavior from logging and telemetry config.
    ///
    /// This function is pure and intentionally avoids touching any global
    /// tracing state so tests can exercise the contract safely.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryInitError`] when strict telemetry is enabled and the
    /// OTLP configuration is incomplete or invalid.
    pub fn from_config(
        log: &LogConfig,
        telemetry: &TelemetryConfig,
        profile: Option<&str>,
    ) -> Result<Self, TelemetryInitError> {
        let log_format = resolve_log_format(log, profile);
        if !telemetry.enabled {
            return Ok(Self {
                log_format,
                trace_export: TraceExport::Disabled,
                warning: None,
            });
        }

        if telemetry.service_name.trim().is_empty() {
            return strict_or_fallback(
                log_format,
                telemetry.strict,
                TelemetryInitError::EmptyServiceName,
            );
        }

        let Some(endpoint) = telemetry
            .otlp_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return strict_or_fallback(
                log_format,
                telemetry.strict,
                TelemetryInitError::MissingEndpoint,
            );
        };

        if let Err(error) = validate_otlp_endpoint(endpoint) {
            return strict_or_fallback(log_format, telemetry.strict, error);
        }

        Ok(Self {
            log_format,
            trace_export: TraceExport::Otlp(OtlpTraceRuntime {
                endpoint: endpoint.to_owned(),
                protocol: telemetry.protocol,
                resource: TelemetryResource {
                    service_name: telemetry.service_name.clone(),
                    service_namespace: telemetry.service_namespace.clone(),
                    service_version: telemetry.service_version.clone(),
                    environment: telemetry.environment.clone(),
                },
            }),
            warning: None,
        })
    }
}

/// Initialize the global tracing subscriber based on Autumn telemetry config.
///
/// # Errors
///
/// Returns [`TelemetryInitError`] when telemetry planning fails or when the
/// tracing subscriber / OTLP exporter cannot be installed.
pub fn init(
    log: &LogConfig,
    telemetry: &TelemetryConfig,
    profile: Option<&str>,
) -> Result<TelemetryGuard, TelemetryInitError> {
    let runtime = TelemetryRuntime::from_config(log, telemetry, profile)?;
    if let Some(warning) = runtime.warning.as_deref() {
        eprintln!("Warning: {warning}");
    }

    match &runtime.trace_export {
        TraceExport::Disabled => init_logging_only(log, runtime.log_format),
        TraceExport::Otlp(otlp) => {
            init_otlp_runtime(log, runtime.log_format, telemetry.strict, otlp)
        }
    }
}

fn strict_or_fallback(
    log_format: ResolvedLogFormat,
    strict: bool,
    error: TelemetryInitError,
) -> Result<TelemetryRuntime, TelemetryInitError> {
    if strict {
        Err(error)
    } else {
        Ok(TelemetryRuntime {
            log_format,
            trace_export: TraceExport::Disabled,
            warning: Some(error.to_string()),
        })
    }
}

fn resolve_log_format(log: &LogConfig, profile: Option<&str>) -> ResolvedLogFormat {
    match log.format {
        LogFormat::Pretty => ResolvedLogFormat::Pretty,
        LogFormat::Json => ResolvedLogFormat::Json,
        LogFormat::Auto => {
            if is_production_profile(profile) || is_production_env() {
                ResolvedLogFormat::Json
            } else {
                ResolvedLogFormat::Pretty
            }
        }
    }
}

fn is_production_profile(profile: Option<&str>) -> bool {
    profile.is_some_and(|value| {
        value.eq_ignore_ascii_case("prod") || value.eq_ignore_ascii_case("production")
    })
}

fn is_production_env() -> bool {
    std::env::var("AUTUMN_ENV").is_ok_and(|value| value.eq_ignore_ascii_case("production"))
}

fn validate_otlp_endpoint(endpoint: &str) -> Result<(), TelemetryInitError> {
    let uri: Uri = endpoint.parse().map_err(|error: http::uri::InvalidUri| {
        TelemetryInitError::InvalidEndpoint {
            endpoint: endpoint.to_owned(),
            reason: error.to_string(),
        }
    })?;

    if uri.scheme().is_none() {
        return Err(TelemetryInitError::InvalidEndpoint {
            endpoint: endpoint.to_owned(),
            reason: "missing URI scheme".to_owned(),
        });
    }

    if uri.authority().is_none() {
        return Err(TelemetryInitError::InvalidEndpoint {
            endpoint: endpoint.to_owned(),
            reason: "missing URI authority".to_owned(),
        });
    }

    Ok(())
}

fn build_filter(log: &LogConfig) -> EnvFilter {
    EnvFilter::try_new(&log.level).unwrap_or_else(|error| {
        eprintln!(
            "Warning: invalid log filter {:?}: {error}, falling back to \"info\"",
            log.level
        );
        EnvFilter::new("info")
    })
}

fn init_logging_only(
    log: &LogConfig,
    log_format: ResolvedLogFormat,
) -> Result<TelemetryGuard, TelemetryInitError> {
    let filter = build_filter(log);
    match log_format {
        ResolvedLogFormat::Json => tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .try_init()
            .map_err(|error| TelemetryInitError::SubscriberInit(error.to_string()))?,
        ResolvedLogFormat::Pretty => tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty())
            .try_init()
            .map_err(|error| TelemetryInitError::SubscriberInit(error.to_string()))?,
    }

    Ok(TelemetryGuard::disabled())
}

#[cfg(feature = "telemetry-otlp")]
fn init_otlp_runtime(
    log: &LogConfig,
    log_format: ResolvedLogFormat,
    strict: bool,
    otlp: &OtlpTraceRuntime,
) -> Result<TelemetryGuard, TelemetryInitError> {
    let provider = match build_tracer_provider(otlp) {
        Ok(provider) => provider,
        Err(error) => {
            if strict {
                return Err(error);
            }
            eprintln!("Warning: {error}");
            return init_logging_only(log, log_format);
        }
    };

    let tracer = provider.tracer("autumn-web");
    let filter = build_filter(log);
    match log_format {
        ResolvedLogFormat::Json => tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init()
            .map_err(|error| TelemetryInitError::SubscriberInit(error.to_string()))?,
        ResolvedLogFormat::Pretty => tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init()
            .map_err(|error| TelemetryInitError::SubscriberInit(error.to_string()))?,
    }

    Ok(TelemetryGuard::with_provider(provider))
}

#[cfg(not(feature = "telemetry-otlp"))]
fn init_otlp_runtime(
    log: &LogConfig,
    log_format: ResolvedLogFormat,
    strict: bool,
    _otlp: &OtlpTraceRuntime,
) -> Result<TelemetryGuard, TelemetryInitError> {
    if strict {
        return Err(TelemetryInitError::FeatureDisabled);
    }

    eprintln!("Warning: {}", TelemetryInitError::FeatureDisabled);
    init_logging_only(log, log_format)
}

#[cfg(feature = "telemetry-otlp")]
fn build_tracer_provider(otlp: &OtlpTraceRuntime) -> Result<SdkTracerProvider, TelemetryInitError> {
    let resource = Resource::builder()
        .with_service_name(otlp.resource.service_name.clone())
        .with_attributes(build_resource_attributes(&otlp.resource))
        .build();

    let exporter = match otlp.protocol {
        TelemetryProtocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(otlp.endpoint.clone())
            .build(),
        TelemetryProtocol::HttpProtobuf => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(otlp.endpoint.clone())
            .build(),
    }
    .map_err(|error| TelemetryInitError::ExporterInit(error.to_string()))?;

    // Install a W3C Trace Context propagator so the `TraceContextLayer`
    // middleware can extract incoming `traceparent` headers and inject
    // the current context into outgoing responses. Uses the global
    // text-map propagator slot maintained by `opentelemetry::global`.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

#[cfg(feature = "telemetry-otlp")]
fn build_resource_attributes(resource: &TelemetryResource) -> [KeyValue; 3] {
    [
        KeyValue::new(
            "service.namespace",
            resource.service_namespace.clone().unwrap_or_default(),
        ),
        KeyValue::new("service.version", resource.service_version.clone()),
        KeyValue::new("deployment.environment", resource.environment.clone()),
    ]
}

// ----------------------------------------------------------------------------
// TelemetryProvider — tier-1 boot-time replaceable telemetry init
// ----------------------------------------------------------------------------

/// Pluggable boot-time telemetry initializer.
///
/// Replace the default `tracing-subscriber + OTLP` initializer with a custom
/// strategy (Datadog tracer, Honeycomb beeline, Sentry breadcrumbs, custom
/// log aggregator) by implementing this trait and installing it on the
/// [`AppBuilder`](crate::app::AppBuilder) via
/// [`with_telemetry_provider`](crate::app::AppBuilder::with_telemetry_provider).
///
/// Initialization is synchronous — the trait mirrors the shape of the
/// underlying [`init`] free function. Custom providers that need async setup
/// can spin up a runtime internally or, more commonly, do their async work
/// from within the returned [`TelemetryGuard`]'s lifecycle hooks.
///
/// # Example
///
/// ```rust,no_run
/// use autumn_web::config::{LogConfig, TelemetryConfig};
/// use autumn_web::telemetry::{TelemetryGuard, TelemetryInitError, TelemetryProvider};
///
/// pub struct DatadogTelemetryProvider;
///
/// impl TelemetryProvider for DatadogTelemetryProvider {
///     fn init(
///         &self,
///         _log: &LogConfig,
///         _telemetry: &TelemetryConfig,
///         _profile: Option<&str>,
///     ) -> Result<TelemetryGuard, TelemetryInitError> {
///         // configure datadog-tracing here, then return a guard whose Drop
///         // cleanly flushes the exporter.
///         Ok(TelemetryGuard::disabled())
///     }
/// }
/// ```
pub trait TelemetryProvider: Send + Sync + 'static {
    /// Initialize tracing/log subscribers and any exporters.
    ///
    /// Returns a [`TelemetryGuard`] whose `Drop` impl is responsible for
    /// flushing exporters and tearing down any background tasks.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryInitError`] if subscriber registration or exporter
    /// setup fails. Propagates to bootstrap and aborts the app.
    fn init(
        &self,
        log: &LogConfig,
        telemetry: &TelemetryConfig,
        profile: Option<&str>,
    ) -> Result<TelemetryGuard, TelemetryInitError>;
}

/// Default [`TelemetryProvider`] — `tracing-subscriber` with optional OTLP export.
///
/// Delegates to the free function [`init`]. This is the provider used when no
/// override is installed via
/// [`with_telemetry_provider`](crate::app::AppBuilder::with_telemetry_provider).
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingOtlpTelemetryProvider;

impl TracingOtlpTelemetryProvider {
    /// Construct a new default telemetry provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TelemetryProvider for TracingOtlpTelemetryProvider {
    fn init(
        &self,
        log: &LogConfig,
        telemetry: &TelemetryConfig,
        profile: Option<&str>,
    ) -> Result<TelemetryGuard, TelemetryInitError> {
        init(log, telemetry, profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No-op provider for tests — returns a disabled guard without touching the
    /// global tracing subscriber. Verifies the trait actually overrides the
    /// default `tracing-subscriber + OTLP` initializer.
    struct NoOpTelemetryProvider;

    impl TelemetryProvider for NoOpTelemetryProvider {
        fn init(
            &self,
            _log: &LogConfig,
            _telemetry: &TelemetryConfig,
            _profile: Option<&str>,
        ) -> Result<TelemetryGuard, TelemetryInitError> {
            Ok(TelemetryGuard::disabled())
        }
    }

    #[test]
    fn telemetry_provider_trait_returns_supplied_guard() {
        let provider = NoOpTelemetryProvider;
        let log = LogConfig::default();
        let telemetry = TelemetryConfig::default();
        // Should succeed and not touch the global subscriber.
        let guard = provider
            .init(&log, &telemetry, Some("test"))
            .expect("no-op provider should succeed");
        // Sanity: the disabled guard must be droppable without panic.
        drop(guard);
    }
    #[test]
    fn build_filter_falls_back_to_info_on_invalid_level() {
        let log = LogConfig {
            level: "this_is_not_a_valid_directive_it_lacks_an_equal_sign_and_is_not_a_level,foo=bar=baz=invalid".to_owned(),
            ..Default::default()
        };

        let filter = build_filter(&log);
        assert_eq!(filter.to_string(), "info");
    }
}
