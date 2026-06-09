//! Structured per-request access log (#999).
//!
//! Emits exactly one `tracing` event (target [`ACCESS_LOG_TARGET`], level
//! `INFO`) at the point each response is returned, carrying the HTTP method,
//! the matched low-cardinality route template (never the raw path), the
//! response status code, the request duration in milliseconds, and the
//! request's `request_id` (the same value used by the `x-request-id` header
//! and error pages).
//!
//! The event flows through the already-installed subscriber, so it honors
//! `LogConfig.format` (`pretty` / `json`) and works with **no** telemetry
//! feature and no OTLP collector. It never includes query strings, headers, or
//! bodies, preserving the log scrubbing posture established for logs (#697) by
//! construction.
//!
//! Access logging is on by default (`log.access_log = true`). Steady-state
//! probe and asset noise is excluded via `log.access_log_exclude`
//! (default: `/health`, `/actuator`, `/static`), matched on whole path
//! segments so `/healthz` is still logged while `/actuator/health` is not.
//!
//! Applied automatically by the framework router, inner to
//! [`RequestIdLayer`](crate::middleware::RequestIdLayer) (so the request id is
//! available) and to the log-context layer (so the event is emitted inside the
//! request span).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::MatchedPath;
use axum::http::{Method, Request, Response};
use pin_project_lite::pin_project;
use tower::{Layer, Service};

use crate::middleware::RequestId;

/// `tracing` target carried by every access-log event, e.g. for filtering
/// (`autumn::access=off`) or routing in a custom layer.
pub const ACCESS_LOG_TARGET: &str = "autumn::access";

/// Route label used when no route matched the request (e.g. 404s), keeping the
/// `route` field low-cardinality. Mirrors the metrics layer.
pub const UNMATCHED_ROUTE: &str = "_unmatched";

/// Tower [`Layer`] that emits one structured access-log event per served
/// request. Applied automatically by the framework router when
/// `log.access_log` is enabled (the default).
#[derive(Clone, Debug)]
pub struct AccessLogLayer {
    exclude: Arc<[String]>,
}

impl AccessLogLayer {
    /// Create a new layer. Requests whose path falls under one of the
    /// `exclude` prefixes (whole-segment match) are not logged.
    #[must_use]
    pub fn new(exclude: Vec<String>) -> Self {
        Self {
            exclude: exclude.into(),
        }
    }
}

impl<S> Layer<S> for AccessLogLayer {
    type Service = AccessLogService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AccessLogService {
            inner,
            exclude: Arc::clone(&self.exclude),
        }
    }
}

/// Tower [`Service`] produced by [`AccessLogLayer`].
#[derive(Clone, Debug)]
pub struct AccessLogService<S> {
    inner: S,
    exclude: Arc<[String]>,
}

/// Request facts captured before the inner service consumes the request.
/// `None` when the request path is excluded from access logging.
struct RequestMeta {
    method: Method,
    route: Option<String>,
    request_id: Option<String>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for AccessLogService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = AccessLogFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let meta = if is_excluded(req.uri().path(), &self.exclude) {
            None
        } else {
            Some(RequestMeta {
                method: req.method().clone(),
                route: req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|matched| matched.as_str().to_owned()),
                request_id: req.extensions().get::<RequestId>().map(ToString::to_string),
            })
        };

        AccessLogFuture {
            inner: self.inner.call(req),
            meta,
            start: Instant::now(),
        }
    }
}

/// Returns `true` when `path` equals an exclusion prefix or lives under it as
/// a whole path segment (`/actuator` excludes `/actuator/health`, not
/// `/actuators`). Trailing slashes on configured prefixes are ignored.
fn is_excluded(path: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches('/');
        !prefix.is_empty()
            && path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

pin_project! {
    /// Future that emits the access-log event once the inner service produces
    /// its response.
    pub struct AccessLogFuture<F> {
        #[pin]
        inner: F,
        meta: Option<RequestMeta>,
        start: Instant,
    }
}

impl<F, ResBody, E> Future for AccessLogFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                if let Some(meta) = this.meta.take() {
                    let duration_ms = this.start.elapsed().as_secs_f64() * 1000.0;
                    tracing::info!(
                        target: ACCESS_LOG_TARGET,
                        method = %meta.method,
                        route = meta.route.as_deref().unwrap_or(UNMATCHED_ROUTE),
                        status = response.status().as_u16(),
                        duration_ms,
                        request_id = meta.request_id.as_deref(),
                        "request served"
                    );
                }
                Poll::Ready(Ok(response))
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclude(prefixes: &[&str]) -> Vec<String> {
        prefixes.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn excludes_exact_path_and_sub_segments() {
        let prefixes = exclude(&["/health", "/actuator", "/static"]);
        assert!(is_excluded("/health", &prefixes));
        assert!(is_excluded("/actuator/health", &prefixes));
        assert!(is_excluded("/static/css/app.css", &prefixes));
    }

    #[test]
    fn does_not_exclude_lookalike_prefixes() {
        let prefixes = exclude(&["/health", "/static"]);
        assert!(!is_excluded("/healthz", &prefixes));
        assert!(!is_excluded("/staticfiles", &prefixes));
        assert!(!is_excluded("/users/1", &prefixes));
    }

    #[test]
    fn trailing_slashes_in_config_are_tolerated() {
        let prefixes = exclude(&["/actuator/"]);
        assert!(is_excluded("/actuator", &prefixes));
        assert!(is_excluded("/actuator/metrics", &prefixes));
    }

    #[test]
    fn empty_or_slash_only_prefixes_exclude_nothing() {
        assert!(!is_excluded("/users/1", &exclude(&[""])));
        assert!(!is_excluded("/users/1", &exclude(&["/"])));
        assert!(!is_excluded("/users/1", &[]));
    }
}
