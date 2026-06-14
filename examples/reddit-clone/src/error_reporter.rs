use autumn_web::reporting::{ErrorEvent, ErrorReporter, ReportFuture};

/// Structured error reporter: emits a `tracing::error!` event for every
/// panic and 5xx so they surface in your log aggregator / APM of choice.
///
/// In production, replace or supplement this with a Sentry SDK call, an
/// OpenTelemetry span event, or a webhook to your on-call channel. The
/// `ErrorEvent` carries the status code, route, request ID, and optional
/// panic payload — everything you need to triage a production incident.
pub struct StructuredReporter;

impl ErrorReporter for StructuredReporter {
    fn report<'a>(&'a self, event: &'a ErrorEvent) -> ReportFuture<'a> {
        Box::pin(async move {
            let kind = if event.panic.is_some() {
                "panic"
            } else {
                "error"
            };
            tracing::error!(
                error.kind = kind,
                http.status = %event.status,
                http.method = event.method.as_deref().unwrap_or("-"),
                http.route  = event.route.as_deref().unwrap_or("-"),
                request_id  = event.request_id.as_deref().unwrap_or("-"),
                "application error — forward to Sentry/Honeycomb in production"
            );
        })
    }
}
