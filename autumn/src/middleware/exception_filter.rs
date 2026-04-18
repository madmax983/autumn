//! Global exception filter middleware.
//!
//! Intercepts error responses produced by [`AutumnError`](crate::AutumnError)
//! and passes them through a chain of user-registered filters before the
//! response is sent to the client.
//!
//! # How it works
//!
//! When `AutumnError::into_response()` runs, it stashes an
//! [`AutumnErrorInfo`] clone into the response extensions. The
//! [`ExceptionFilterLayer`] middleware checks for this extension after the
//! inner service returns. If present, it runs the filter chain, giving each
//! filter a chance to transform, log, or replace the response.
//!
//! # Examples
//!
//! ```rust,no_run
//! use autumn_web::middleware::ExceptionFilter;
//! use autumn_web::middleware::AutumnErrorInfo;
//! use axum::response::Response;
//!
//! struct LoggingFilter;
//!
//! impl ExceptionFilter for LoggingFilter {
//!     fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
//!         eprintln!("Error {}: {}", error.status, error.message);
//!         response
//!     }
//! }
//!
//! # #[autumn_web::main]
//! # async fn main() {
//! autumn_web::app()
//!     .exception_filter(LoggingFilter)
//!     // .routes(...)
//! #   .routes(vec![])
//! #   ;
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use pin_project_lite::pin_project;
use tower::{Layer, Service};

/// Metadata extracted from an [`AutumnError`](crate::AutumnError) and stashed
/// in the response extensions.
///
/// Exception filters receive this to inspect the original error without
/// needing to parse the response body.
#[derive(Clone, Debug)]
pub struct AutumnErrorInfo {
    /// The HTTP status code of the error.
    pub status: StatusCode,
    /// The human-readable error message.
    pub message: String,
    /// Optional field-level validation details (for 422 responses).
    pub details: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl AutumnErrorInfo {
    /// Build the default JSON error response from this info.
    ///
    /// Useful when a filter wants to log or enrich but keep the standard
    /// response format.
    #[must_use]
    pub fn into_default_response(self) -> Response {
        let body = serde_json::json!({
            "error": {
                "status": self.status.as_u16(),
                "message": self.message,
                "details": self.details,
            }
        });
        (self.status, axum::Json(body)).into_response()
    }
}

/// Trait for global exception filters.
///
/// Implement this trait to intercept error responses before they are sent
/// to the client. Filters can log, transform, or completely replace the
/// response.
///
/// # Examples
///
/// ```rust
/// use autumn_web::middleware::{ExceptionFilter, AutumnErrorInfo};
/// use axum::response::Response;
///
/// struct SentryFilter;
///
/// impl ExceptionFilter for SentryFilter {
///     fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
///         // Log to Sentry, metrics, etc.
///         eprintln!("[sentry] {} {}", error.status, error.message);
///         response // pass through unchanged
///     }
/// }
/// ```
pub trait ExceptionFilter: Send + Sync + 'static {
    /// Inspect and optionally transform an error response.
    ///
    /// `error` contains the original error metadata. `response` is the
    /// current HTTP response (which may have been modified by a previous
    /// filter in the chain). Return the response to send to the client.
    fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response;
}

/// Tower [`Layer`] that applies the exception filter chain.
///
/// Applied automatically by [`AppBuilder::run`](crate::app::AppBuilder::run)
/// when at least one exception filter is registered.
#[derive(Clone)]
pub struct ExceptionFilterLayer {
    filters: Arc<Vec<Arc<dyn ExceptionFilter>>>,
}

impl ExceptionFilterLayer {
    /// Create a new layer with the given filter chain.
    #[must_use]
    pub fn new(filters: Vec<Arc<dyn ExceptionFilter>>) -> Self {
        Self {
            filters: Arc::new(filters),
        }
    }
}

impl<S> Layer<S> for ExceptionFilterLayer {
    type Service = ExceptionFilterService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ExceptionFilterService {
            inner,
            filters: Arc::clone(&self.filters),
        }
    }
}

/// Tower [`Service`] produced by [`ExceptionFilterLayer`].
#[derive(Clone)]
pub struct ExceptionFilterService<S> {
    inner: S,
    filters: Arc<Vec<Arc<dyn ExceptionFilter>>>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for ExceptionFilterService<S>
where
    S: Service<Request<ReqBody>, Response = Response>,
{
    type Response = Response;
    type Error = S::Error;
    type Future = ExceptionFilterFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        ExceptionFilterFuture {
            inner: self.inner.call(req),
            filters: Arc::clone(&self.filters),
        }
    }
}

pin_project! {
    /// Future that applies exception filters to error responses.
    pub struct ExceptionFilterFuture<F> {
        #[pin]
        inner: F,
        filters: Arc<Vec<Arc<dyn ExceptionFilter>>>,
    }
}

impl<F, E> Future for ExceptionFilterFuture<F>
where
    F: Future<Output = Result<Response, E>>,
{
    type Output = Result<Response, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                // Check if this response came from AutumnError and clone the info out
                if let Some(error_info) = response.extensions().get::<AutumnErrorInfo>().cloned() {
                    let mut response = response;
                    let filters = this.filters;
                    for filter in filters.iter() {
                        response = filter.filter(&error_info, response);
                    }

                    if axum::body::HttpBody::size_hint(response.body()).exact() == Some(0) {
                        let mut default_resp = error_info.into_default_response();
                        if let Some(wants_html) = response
                            .extensions()
                            .get::<crate::middleware::error_page_filter::WantsHtml>()
                            .cloned()
                        {
                            default_resp.extensions_mut().insert(wants_html);
                        }
                        if let Some(req_ctx) = response
                            .extensions()
                            .get::<crate::middleware::error_page_filter::ErrorPageRequestContext>()
                            .cloned()
                        {
                            default_resp.extensions_mut().insert(req_ctx);
                        }
                        response = default_resp;
                    }

                    Poll::Ready(Ok(response))
                } else {
                    Poll::Ready(Ok(response))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use http::Request;
    use tower::ServiceExt;

    use crate::error::AutumnError;

    #[tokio::test]
    async fn filter_receives_error_info() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static CALLED: AtomicBool = AtomicBool::new(false);

        struct TestFilter;
        impl ExceptionFilter for TestFilter {
            fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
                assert_eq!(error.status, StatusCode::NOT_FOUND);
                assert_eq!(error.message, "not here");
                CALLED.store(true, Ordering::SeqCst);
                response
            }
        }

        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    Err::<String, AutumnError>(AutumnError::not_found_msg("not here"))
                }),
            )
            .layer(ExceptionFilterLayer::new(vec![Arc::new(TestFilter)]));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(CALLED.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn filter_can_replace_response() {
        struct ReplaceFilter;
        impl ExceptionFilter for ReplaceFilter {
            fn filter(&self, _error: &AutumnErrorInfo, _response: Response) -> Response {
                (StatusCode::SERVICE_UNAVAILABLE, "custom error page").into_response()
            }
        }

        let app = Router::new()
            .route(
                "/",
                get(|| async { Err::<String, AutumnError>(AutumnError::not_found_msg("gone")) }),
            )
            .layer(ExceptionFilterLayer::new(vec![Arc::new(ReplaceFilter)]));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"custom error page");
    }

    #[tokio::test]
    async fn success_responses_bypass_filters() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static CALLED: AtomicBool = AtomicBool::new(false);

        struct NeverFilter;
        impl ExceptionFilter for NeverFilter {
            fn filter(&self, _error: &AutumnErrorInfo, response: Response) -> Response {
                CALLED.store(true, Ordering::SeqCst);
                response
            }
        }

        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(ExceptionFilterLayer::new(vec![Arc::new(NeverFilter)]));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!CALLED.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn multiple_filters_run_in_order() {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        struct OrderFilter(u32);
        impl ExceptionFilter for OrderFilter {
            fn filter(&self, _error: &AutumnErrorInfo, response: Response) -> Response {
                let current = COUNTER.fetch_add(1, Ordering::SeqCst);
                assert_eq!(current, self.0, "filters should run in registration order");
                response
            }
        }

        COUNTER.store(0, Ordering::SeqCst);

        let app = Router::new()
            .route(
                "/",
                get(|| async { Err::<String, AutumnError>(AutumnError::bad_request_msg("oops")) }),
            )
            .layer(ExceptionFilterLayer::new(vec![
                Arc::new(OrderFilter(0)),
                Arc::new(OrderFilter(1)),
                Arc::new(OrderFilter(2)),
            ]));

        app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(COUNTER.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn error_info_into_default_response() {
        let info = AutumnErrorInfo {
            status: StatusCode::NOT_FOUND,
            message: "not found".into(),
            details: None,
        };
        let response = info.into_default_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fallback_404_produces_empty_body_and_preserves_extensions() {
        use crate::middleware::error_page_filter::{ErrorPageRequestContext, WantsHtml};
        use axum::http::Request;
        use tower::ServiceExt;

        struct InjectExtensionFilter;
        impl ExceptionFilter for InjectExtensionFilter {
            fn filter(&self, _error: &AutumnErrorInfo, mut response: Response) -> Response {
                response.extensions_mut().insert(WantsHtml(true));
                response.extensions_mut().insert(ErrorPageRequestContext {
                    uri: "/missing".parse().unwrap(),
                    request_id: None,
                });
                response
            }
        }

        // This simulates a request entering the ExceptionFilterLayer and being assigned
        // the extensions WantsHtml and ErrorPageRequestContext, which should be preserved
        // across the default JSON body fallback.
        let app = Router::new()
            .fallback(crate::middleware::error_page_filter::fallback_404_handler)
            .layer(ExceptionFilterLayer::new(vec![Arc::new(
                InjectExtensionFilter,
            )]));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.extensions().get::<WantsHtml>().is_some());
        assert!(
            response
                .extensions()
                .get::<ErrorPageRequestContext>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn fallback_404_produces_empty_body_and_preserves_only_one_extension() {
        use crate::middleware::error_page_filter::WantsHtml;
        use axum::http::Request;
        use tower::ServiceExt;

        struct InjectExtensionFilter;
        impl ExceptionFilter for InjectExtensionFilter {
            fn filter(&self, _error: &AutumnErrorInfo, mut response: Response) -> Response {
                response.extensions_mut().insert(WantsHtml(true));
                response
            }
        }

        let app = Router::new()
            .fallback(crate::middleware::error_page_filter::fallback_404_handler)
            .layer(ExceptionFilterLayer::new(vec![Arc::new(
                InjectExtensionFilter,
            )]));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.extensions().get::<WantsHtml>().is_some());
        assert!(
            response
                .extensions()
                .get::<crate::middleware::error_page_filter::ErrorPageRequestContext>()
                .is_none()
        );
    }

    #[tokio::test]
    async fn fallback_404_produces_empty_body_and_preserves_the_other_extension() {
        use crate::middleware::error_page_filter::ErrorPageRequestContext;
        use axum::http::Request;
        use tower::ServiceExt;

        struct InjectExtensionFilter;
        impl ExceptionFilter for InjectExtensionFilter {
            fn filter(&self, _error: &AutumnErrorInfo, mut response: Response) -> Response {
                response.extensions_mut().insert(ErrorPageRequestContext {
                    uri: "/missing".parse().unwrap(),
                    request_id: None,
                });
                response
            }
        }

        let app = Router::new()
            .fallback(crate::middleware::error_page_filter::fallback_404_handler)
            .layer(ExceptionFilterLayer::new(vec![Arc::new(
                InjectExtensionFilter,
            )]));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(
            response
                .extensions()
                .get::<crate::middleware::error_page_filter::WantsHtml>()
                .is_none()
        );
        assert!(
            response
                .extensions()
                .get::<ErrorPageRequestContext>()
                .is_some()
        );
    }
}
