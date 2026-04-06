//! Metrics middleware -- records request count, latency, and status code distribution.
//!
//! The [`MetricsLayer`] is applied automatically by the framework when building
//! the router. It instruments every request and records metrics into a shared
//! [`MetricsCollector`] stored in `AppState`.
//!
//! Metrics are exposed via the `/actuator/metrics` endpoint.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::MatchedPath;
use axum::http::{Request, Response};
use pin_project_lite::pin_project;
use serde::Serialize;
use tower::{Layer, Service};

/// Shared metrics collector that aggregates request statistics.
#[derive(Debug, Clone)]
pub struct MetricsCollector {
    inner: Arc<MetricsInner>,
}

#[derive(Debug)]
struct MetricsInner {
    /// Total number of requests received.
    requests_total: AtomicU64,
    /// Currently active (in-flight) requests.
    requests_active: AtomicU64,
    /// Per-route metrics: key is "METHOD /path".
    by_route: RwLock<HashMap<String, RouteMetrics>>,
    /// Status code buckets: 2xx, 3xx, 4xx, 5xx.
    by_status: StatusBuckets,
    /// Global latency samples (bounded ring buffer).
    latencies_ms: RwLock<VecDeque<u64>>,
}

#[derive(Debug, Default)]
#[allow(clippy::struct_field_names)]
struct StatusBuckets {
    status_2xx: AtomicU64,
    status_3xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
}

#[derive(Debug, Clone)]
struct RouteMetrics {
    count: u64,
    latencies_ms: VecDeque<u64>,
}

impl Default for RouteMetrics {
    fn default() -> Self {
        Self {
            count: 0,
            latencies_ms: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
        }
    }
}

/// Maximum number of latency samples to keep per route.
const MAX_LATENCY_SAMPLES: usize = 10_000;

impl MetricsCollector {
    /// Create a new empty metrics collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                requests_total: AtomicU64::new(0),
                requests_active: AtomicU64::new(0),
                by_route: RwLock::new(HashMap::new()),
                by_status: StatusBuckets::default(),
                latencies_ms: RwLock::new(VecDeque::with_capacity(MAX_LATENCY_SAMPLES)),
            }),
        }
    }

    /// Record a completed request.
    pub fn record(&self, method: &str, route: &str, status: u16, latency_ms: u64) {
        self.inner.requests_total.fetch_add(1, Ordering::Relaxed);

        // Status bucket
        match status / 100 {
            2 => self
                .inner
                .by_status
                .status_2xx
                .fetch_add(1, Ordering::Relaxed),
            3 => self
                .inner
                .by_status
                .status_3xx
                .fetch_add(1, Ordering::Relaxed),
            4 => self
                .inner
                .by_status
                .status_4xx
                .fetch_add(1, Ordering::Relaxed),
            5 => self
                .inner
                .by_status
                .status_5xx
                .fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };

        // Global latency
        if let Ok(mut latencies) = self.inner.latencies_ms.write() {
            if latencies.len() >= MAX_LATENCY_SAMPLES {
                latencies.pop_front();
            }
            latencies.push_back(latency_ms);
        }

        // Per-route
        let key = format!("{method} {route}");
        if let Ok(mut routes) = self.inner.by_route.write() {
            let entry = routes.entry(key).or_default();
            entry.count += 1;
            if entry.latencies_ms.len() >= MAX_LATENCY_SAMPLES {
                entry.latencies_ms.pop_front();
            }
            entry.latencies_ms.push_back(latency_ms);
        }
    }

    fn increment_active(&self) {
        self.inner.requests_active.fetch_add(1, Ordering::Relaxed);
    }

    fn decrement_active(&self) {
        self.inner.requests_active.fetch_sub(1, Ordering::Relaxed);
    }

    /// Produce a snapshot of current metrics for the `/actuator/metrics` endpoint.
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        let global_latency = self
            .inner
            .latencies_ms
            .read()
            .map(|v| compute_percentiles(&v))
            .unwrap_or_default();

        let by_route = self
            .inner
            .by_route
            .read()
            .map(|routes| {
                routes
                    .iter()
                    .map(|(k, v)| {
                        let pcts = compute_percentiles(&v.latencies_ms);
                        (
                            k.clone(),
                            RouteSnapshot {
                                count: v.count,
                                p50_ms: pcts.p50,
                                p95_ms: pcts.p95,
                                p99_ms: pcts.p99,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        MetricsSnapshot {
            http: HttpMetrics {
                requests_total: self.inner.requests_total.load(Ordering::Relaxed),
                requests_active: self.inner.requests_active.load(Ordering::Relaxed),
                latency_ms: LatencySnapshot {
                    p50: global_latency.p50,
                    p95: global_latency.p95,
                    p99: global_latency.p99,
                },
                by_route,
                by_status: StatusSnapshot {
                    s2xx: self.inner.by_status.status_2xx.load(Ordering::Relaxed),
                    s3xx: self.inner.by_status.status_3xx.load(Ordering::Relaxed),
                    s4xx: self.inner.by_status.status_4xx.load(Ordering::Relaxed),
                    s5xx: self.inner.by_status.status_5xx.load(Ordering::Relaxed),
                },
            },
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable metrics snapshot returned by `/actuator/metrics`.
#[derive(Serialize)]
pub struct MetricsSnapshot {
    /// HTTP-specific metrics including latency and status codes.
    pub http: HttpMetrics,
}

/// A snapshot of HTTP metrics across the entire application.
#[derive(Serialize)]
pub struct HttpMetrics {
    /// Total number of requests processed.
    pub requests_total: u64,
    /// Number of currently active (in-flight) requests.
    pub requests_active: u64,
    /// Global latency percentiles (in milliseconds).
    pub latency_ms: LatencySnapshot,
    /// Metrics broken down by route ("METHOD /path").
    pub by_route: HashMap<String, RouteSnapshot>,
    /// Global distribution of HTTP status codes.
    pub by_status: StatusSnapshot,
}

/// Percentiles for latency measurements.
#[derive(Serialize, Default)]
pub struct LatencySnapshot {
    /// 50th percentile (median) latency in milliseconds.
    pub p50: u64,
    /// 95th percentile latency in milliseconds.
    pub p95: u64,
    /// 99th percentile latency in milliseconds.
    pub p99: u64,
}

/// Metrics for a specific route.
#[derive(Serialize)]
pub struct RouteSnapshot {
    /// Total number of requests to this route.
    pub count: u64,
    /// 50th percentile (median) latency for this route in milliseconds.
    pub p50_ms: u64,
    /// 95th percentile latency for this route in milliseconds.
    pub p95_ms: u64,
    /// 99th percentile latency for this route in milliseconds.
    pub p99_ms: u64,
}

/// Global distribution of HTTP status codes.
#[derive(Serialize)]
pub struct StatusSnapshot {
    /// Number of 2xx success responses.
    #[serde(rename = "2xx")]
    pub s2xx: u64,
    /// Number of 3xx redirection responses.
    #[serde(rename = "3xx")]
    pub s3xx: u64,
    /// Number of 4xx client error responses.
    #[serde(rename = "4xx")]
    pub s4xx: u64,
    /// Number of 5xx server error responses.
    #[serde(rename = "5xx")]
    pub s5xx: u64,
}

#[derive(Default)]
struct Percentiles {
    p50: u64,
    p95: u64,
    p99: u64,
}

fn compute_percentiles(latencies: &VecDeque<u64>) -> Percentiles {
    let len = latencies.len();
    if len == 0 {
        return Percentiles::default();
    }

    // Pre-allocate to exact capacity and use fast slice copying instead of iterating
    let mut data = Vec::with_capacity(len);
    let (slice1, slice2) = latencies.as_slices();
    data.extend_from_slice(slice1);
    data.extend_from_slice(slice2);

    let p50_idx = len * 50 / 100;
    let p95_idx = len.saturating_sub(1).min(len * 95 / 100);
    let p99_idx = len.saturating_sub(1).min(len * 99 / 100);

    // Use select_nth_unstable to find percentiles in O(N) time instead of O(N log N) sort
    let (_, &mut p99, _) = data.select_nth_unstable(p99_idx);

    // We only need to search the left partition for p95 since p95_idx <= p99_idx
    let (_, &mut p95, _) = data[..=p99_idx].select_nth_unstable(p95_idx);

    // We only need to search the left partition for p50 since p50_idx <= p95_idx
    let (_, &mut p50, _) = data[..=p95_idx].select_nth_unstable(p50_idx);

    Percentiles { p50, p95, p99 }
}

// ── Tower Layer / Service ───────────────────────────────────────

/// Tower [`Layer`] that wraps a service with [`MetricsService`].
///
/// Records request count, latency, active connections, and status code
/// distribution into a shared [`MetricsCollector`].
#[derive(Clone)]
pub struct MetricsLayer {
    collector: MetricsCollector,
}

impl MetricsLayer {
    /// Create a new metrics layer backed by the given collector.
    #[must_use]
    pub const fn new(collector: MetricsCollector) -> Self {
        Self { collector }
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            collector: self.collector.clone(),
        }
    }
}

/// Tower [`Service`] produced by [`MetricsLayer`].
#[derive(Clone)]
pub struct MetricsService<S> {
    inner: S,
    collector: MetricsCollector,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for MetricsService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = MetricsFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let method = req.method().to_string();
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| "_unmatched".to_owned(), |p| p.as_str().to_owned());

        self.collector.increment_active();

        MetricsFuture {
            inner: self.inner.call(req),
            collector: Some(self.collector.clone()),
            method: Some(method),
            route: Some(route),
            start: Instant::now(),
        }
    }
}

pin_project! {
    /// Future that records metrics after the inner service completes.
    pub struct MetricsFuture<F> {
        #[pin]
        inner: F,
        collector: Option<MetricsCollector>,
        method: Option<String>,
        route: Option<String>,
        start: Instant,
    }
}

impl<F, ResBody, E> Future for MetricsFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                if let Some(collector) = this.collector.take() {
                    let latency_ms =
                        u64::try_from(this.start.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let method = this.method.take().unwrap_or_default();
                    let route = this.route.take().unwrap_or_default();
                    let status = response.status().as_u16();
                    collector.record(&method, &route, status, latency_ms);
                    collector.decrement_active();
                }
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(e)) => {
                if let Some(collector) = this.collector.take() {
                    collector.decrement_active();
                }
                Poll::Ready(Err(e))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collector_records_request() {
        let collector = MetricsCollector::new();
        collector.record("GET", "/test", 200, 10);
        collector.record("GET", "/test", 200, 20);
        collector.record("POST", "/test", 500, 50);

        let snap = collector.snapshot();
        assert_eq!(snap.http.requests_total, 3);
        assert_eq!(snap.http.by_status.s2xx, 2);
        assert_eq!(snap.http.by_status.s5xx, 1);
        assert!(snap.http.by_route.contains_key("GET /test"));
        assert_eq!(snap.http.by_route["GET /test"].count, 2);
    }

    #[test]
    fn empty_collector_snapshot() {
        let collector = MetricsCollector::new();
        let snap = collector.snapshot();
        assert_eq!(snap.http.requests_total, 0);
        assert_eq!(snap.http.requests_active, 0);
        assert_eq!(snap.http.latency_ms.p50, 0);
    }

    #[test]
    fn percentiles_computed_correctly() {
        let latencies: VecDeque<u64> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10].into();
        let pcts = compute_percentiles(&latencies);
        assert_eq!(pcts.p50, 6); // sorted[10*50/100] = sorted[5] = 6
        assert_eq!(pcts.p99, 10); // sorted[min(9, 10*99/100)] = sorted[9] = 10
    }

    #[test]
    fn active_connection_tracking() {
        let collector = MetricsCollector::new();
        collector.increment_active();
        collector.increment_active();
        assert_eq!(collector.inner.requests_active.load(Ordering::Relaxed), 2);
        collector.decrement_active();
        assert_eq!(collector.inner.requests_active.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn metrics_layer_records_requests() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        let collector = MetricsCollector::new();
        let app = Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(MetricsLayer::new(collector.clone()));

        let request = Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let snap = collector.snapshot();
        assert_eq!(snap.http.requests_total, 1);
        assert_eq!(snap.http.requests_active, 0);
        assert_eq!(snap.http.by_status.s2xx, 1);

        let route_key = "GET /test";
        assert!(snap.http.by_route.contains_key(route_key));
        assert_eq!(snap.http.by_route[route_key].count, 1);
    }

    #[tokio::test]
    async fn metrics_layer_handles_errors() {
        use axum::body::Body;
        use std::task::{Context, Poll};
        use tower::{Service, ServiceExt};

        // A custom service that fails instead of returning a response
        #[derive(Clone)]
        struct FailingService;

        impl Service<Request<Body>> for FailingService {
            type Response = Response<Body>;
            type Error = std::io::Error;
            type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }

            fn call(&mut self, _req: Request<Body>) -> Self::Future {
                std::future::ready(Err(std::io::Error::other("boom")))
            }
        }

        let collector = MetricsCollector::new();
        let mut svc = MetricsLayer::new(collector.clone()).layer(FailingService);

        // Active should be 0 initially
        assert_eq!(collector.inner.requests_active.load(Ordering::Relaxed), 0);

        let request = Request::builder()
            .method("GET")
            .uri("/fail")
            .body(Body::empty())
            .unwrap();

        // We know it will error
        let result = svc.ready().await.unwrap().call(request).await;
        assert!(result.is_err());

        // Ensure that even though it errored, the active connection count was decremented
        assert_eq!(collector.inner.requests_active.load(Ordering::Relaxed), 0);
    }
}
