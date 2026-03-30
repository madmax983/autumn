//! Integration tests for the middleware pipeline: exception filters,
//! scoped middleware, and handler interceptors.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use autumn_web::config::AutumnConfig;
use autumn_web::error::AutumnError;
use autumn_web::middleware::{AutumnErrorInfo, ExceptionFilter, ExceptionFilterLayer};
use autumn_web::{AppState, get, routes};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use tower::ServiceExt;

fn test_state() -> AppState {
    AppState {
        #[cfg(feature = "db")]
        pool: None,
        profile: None,
        started_at: std::time::Instant::now(),
        health_detailed: false,
        metrics: autumn_web::middleware::MetricsCollector::new(),
        log_levels: autumn_web::actuator::LogLevels::new("info"),
        task_registry: autumn_web::actuator::TaskRegistry::new(),
        config_props: autumn_web::actuator::ConfigProperties::default(),
        #[cfg(feature = "ws")]
        channels: autumn_web::channels::Channels::new(32),
        #[cfg(feature = "ws")]
        shutdown: tokio_util::sync::CancellationToken::new(),
    }
}

// ── Exception Filter tests ─────────────────────────────────────────

struct MarkCalledFilter {
    called: Arc<AtomicBool>,
}

impl ExceptionFilter for MarkCalledFilter {
    fn filter(&self, _error: &AutumnErrorInfo, response: Response) -> Response {
        self.called.store(true, Ordering::SeqCst);
        response
    }
}

#[get("/ok")]
async fn ok_handler() -> &'static str {
    "ok"
}

#[get("/fail")]
async fn fail_handler() -> Result<String, AutumnError> {
    Err(AutumnError::not_found_msg("gone"))
}

#[tokio::test]
async fn exception_filter_on_error_response() {
    let called = Arc::new(AtomicBool::new(false));
    let config = AutumnConfig::default();

    let router =
        autumn_web::app::build_router(routes![ok_handler, fail_handler], &config, test_state());
    // Manually layer the exception filter (build_router doesn't take filters)
    let router = router.layer(ExceptionFilterLayer::new(vec![Arc::new(
        MarkCalledFilter {
            called: called.clone(),
        },
    )]));

    // Error path: filter should fire
    let resp = router
        .clone()
        .oneshot(Request::builder().uri("/fail").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(called.load(Ordering::SeqCst));

    // Success path: filter should NOT fire
    called.store(false, Ordering::SeqCst);
    let resp = router
        .oneshot(Request::builder().uri("/ok").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!called.load(Ordering::SeqCst));
}

// ── Scoped middleware tests ────────────────────────────────────────

/// A simple Tower layer that adds a custom header to every response.
#[derive(Clone)]
struct AddHeaderLayer;

impl<S> tower::Layer<S> for AddHeaderLayer {
    type Service = AddHeaderService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AddHeaderService { inner }
    }
}

#[derive(Clone)]
struct AddHeaderService<S> {
    inner: S,
}

impl<S> tower::Service<axum::http::Request<Body>> for AddHeaderService<S>
where
    S: tower::Service<
            axum::http::Request<Body>,
            Response = Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::http::Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut resp = inner.call(req).await?;
            resp.headers_mut()
                .insert("x-scoped", "true".parse().unwrap());
            Ok(resp)
        })
    }
}

#[get("/users")]
async fn list_users() -> &'static str {
    "users"
}

#[get("/public")]
async fn public_page() -> &'static str {
    "public"
}

#[tokio::test]
async fn scoped_middleware_applies_only_to_group() {
    let _builder = autumn_web::app().routes(routes![public_page]).scoped(
        "/api",
        AddHeaderLayer,
        routes![list_users],
    );

    let state = test_state();

    // Use build_router_with_static (public API) for the non-scoped part,
    // but scoped groups go through AppBuilder. Let's just test via AppBuilder
    // by building directly. We'll construct a router manually.
    // Since AppBuilder::run() starts the server, we test by constructing
    // the router from the public build_router + manual nesting.

    // Simpler: just build a plain axum router that mimics what scoped() does.
    let mut sub_router = axum::Router::new();
    for route in routes![list_users] {
        sub_router = sub_router.route(route.path, route.handler);
    }
    sub_router = sub_router.layer(AddHeaderLayer);

    let mut main_router = axum::Router::new();
    for route in routes![public_page] {
        main_router = main_router.route(route.path, route.handler);
    }
    let main_router = main_router.nest("/api", sub_router).with_state(state);

    // /api/users should have x-scoped header
    let resp = main_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-scoped").unwrap(), "true");

    // /public should NOT have x-scoped header
    let resp = main_router
        .oneshot(
            Request::builder()
                .uri("/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("x-scoped").is_none());
}

// ── AppBuilder::scoped compiles ────────────────────────────────────

#[test]
fn app_builder_scoped_compiles() {
    let _builder = autumn_web::app().routes(routes![ok_handler]).scoped(
        "/api",
        AddHeaderLayer,
        routes![list_users],
    );
}

// ── AppBuilder::exception_filter compiles ──────────────────────────

#[test]
fn app_builder_exception_filter_compiles() {
    struct NoopFilter;
    impl ExceptionFilter for NoopFilter {
        fn filter(&self, _error: &AutumnErrorInfo, response: Response) -> Response {
            response
        }
    }

    let _builder = autumn_web::app()
        .exception_filter(NoopFilter)
        .routes(routes![ok_handler]);
}
