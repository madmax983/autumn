//! Request-scoped log context middleware.
//!
//! Establishes an always-on [`LogContext`](crate::log::context::LogContext) for
//! every HTTP request, seeded with the request's `request_id` (read from the
//! [`RequestId`](crate::middleware::RequestId) extension installed by
//! [`RequestIdLayer`](crate::middleware::RequestIdLayer)). The request is then
//! driven inside both that task-local context and a `tracing` span carrying
//! `request_id`/`user_id`/`tenant_id`, so every event emitted during the
//! request automatically correlates back to it.
//!
//! This layer is **not** gated behind any telemetry feature — it is applied on
//! the default ingress path. Place it inner to `RequestIdLayer` so the request
//! id is available.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{Request, Response};
use tower::{Layer, Service};
use tracing::Instrument as _;

use crate::log::context::{self, LogContext};
use crate::log::filter::ParameterFilter;
use crate::middleware::RequestId;

/// Tower [`Layer`] that installs a [`LogContext`] for the duration of each
/// request. Applied automatically by the framework router.
#[derive(Clone)]
pub struct LogContextLayer {
    filter: Arc<ParameterFilter>,
}

impl LogContextLayer {
    /// Create a new layer using `filter` to scrub sensitive custom fields
    /// recorded via [`with_log_field`](crate::log::context::with_log_field).
    #[must_use]
    pub const fn new(filter: Arc<ParameterFilter>) -> Self {
        Self { filter }
    }
}

impl std::fmt::Debug for LogContextLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogContextLayer").finish_non_exhaustive()
    }
}

impl<S> Layer<S> for LogContextLayer {
    type Service = LogContextService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LogContextService {
            inner,
            filter: Arc::clone(&self.filter),
        }
    }
}

/// Tower [`Service`] produced by [`LogContextLayer`].
#[derive(Clone)]
pub struct LogContextService<S> {
    inner: S,
    filter: Arc<ParameterFilter>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for LogContextService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        tokio::task::futures::TaskLocalFuture<LogContext, tracing::instrument::Instrumented<S::Future>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let request_id = req
            .extensions()
            .get::<RequestId>()
            .map(ToString::to_string);

        // Open an always-on request span carrying the well-known correlation
        // fields. They are declared up front (Empty) so `set_user_id` /
        // `set_tenant_id` can record them later and they surface in standard
        // log output for every event emitted within the request.
        let span = tracing::info_span!(
            "request",
            request_id = tracing::field::Empty,
            user_id = tracing::field::Empty,
            tenant_id = tracing::field::Empty,
        );
        if let Some(ref rid) = request_id {
            span.record("request_id", tracing::field::display(rid));
        }

        let ctx = LogContext::with_filter(request_id, Arc::clone(&self.filter));
        let fut = self.inner.call(req).instrument(span);
        context::scoped(ctx, fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::context;
    use crate::middleware::RequestIdLayer;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use std::sync::Mutex;
    use tower::ServiceExt as _;
    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::Layer as _;

    /// Test tracing layer that records the request context snapshot observed at
    /// the moment each event is emitted.
    #[derive(Clone, Default)]
    struct CaptureLayer {
        seen: Arc<Mutex<Vec<context::LogFields>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for CaptureLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            _event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if let Some(fields) = context::snapshot() {
                self.seen.lock().unwrap().push(fields);
            }
        }
    }

    fn filter() -> Arc<ParameterFilter> {
        Arc::new(ParameterFilter::default())
    }

    #[test]
    fn event_in_handler_carries_request_id_and_user_id() {
        let capture = CaptureLayer::default();
        let seen = Arc::clone(&capture.seen);
        let subscriber = tracing_subscriber::registry().with(capture.boxed());

        with_default(subscriber, || {
            // Build the same layering the framework uses: RequestId outer,
            // LogContext inner.
            let app = Router::new()
                .route(
                    "/",
                    get(|| async {
                        // Simulate the auth layer resolving the user.
                        context::set_user_id("42");
                        tracing::info!("handled");
                        "ok"
                    }),
                )
                .layer(LogContextLayer::new(filter()))
                .layer(RequestIdLayer);

            // `with_default` is sync; drive the request to completion on a local
            // runtime so the captured events are observed under this subscriber.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let response = rt.block_on(async {
                app.oneshot(
                    Request::builder().uri("/").body(Body::empty()).unwrap(),
                )
                .await
                .unwrap()
            });

            let header_id = response
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();

            let events = seen.lock().unwrap();
            let handled = events
                .iter()
                .find(|f| f.request_id.is_some())
                .expect("expected at least one event carrying request context");
            assert_eq!(handled.request_id.as_deref(), Some(header_id.as_str()));
            assert_eq!(handled.user_id.as_deref(), Some("42"));
        });
    }

    #[tokio::test]
    async fn context_is_isolated_between_requests() {
        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    let before = context::snapshot().unwrap();
                    assert!(before.fields.is_empty());
                    context::with_log_field("k", "v");
                    "ok"
                }),
            )
            .layer(LogContextLayer::new(filter()))
            .layer(RequestIdLayer);

        for _ in 0..2 {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::OK);
        }
    }

    #[test]
    fn custom_fields_are_captured_and_sensitive_ones_scrubbed() {
        let capture = CaptureLayer::default();
        let seen = Arc::clone(&capture.seen);
        let subscriber = tracing_subscriber::registry().with(capture.boxed());

        with_default(subscriber, || {
            let app = Router::new()
                .route(
                    "/",
                    get(|| async {
                        context::set_tenant_id("acme");
                        context::with_log_field("order_id", "A-1001");
                        context::with_log_field("password", "hunter2");
                        tracing::info!("processed order");
                        "ok"
                    }),
                )
                .layer(LogContextLayer::new(filter()))
                .layer(RequestIdLayer);

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                    .await
                    .unwrap()
            });

            let events = seen.lock().unwrap();
            let fields = events
                .iter()
                .find(|f| f.fields.contains_key("order_id"))
                .expect("expected an event carrying the custom field");
            assert_eq!(fields.tenant_id.as_deref(), Some("acme"));
            assert_eq!(fields.fields.get("order_id").map(String::as_str), Some("A-1001"));
            assert_eq!(
                fields.fields.get("password").map(String::as_str),
                Some(crate::log::filter::FILTERED_PLACEHOLDER),
                "sensitive custom field should be scrubbed by the layer filter"
            );
        });
    }

    #[tokio::test]
    async fn request_id_matches_x_request_id_header() {
        async fn handler() -> String {
            context::snapshot()
                .and_then(|s| s.request_id)
                .unwrap_or_default()
        }

        let app = Router::new()
            .route("/", get(handler))
            .layer(LogContextLayer::new(filter()))
            .layer(RequestIdLayer);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let header_id = resp
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_id = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_id, header_id);
        assert!(!body_id.is_empty());
    }
}
