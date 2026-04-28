use autumn_web::security::RateLimitConfig;
use autumn_web::security::RateLimitLayer;
use axum::body::Body;
use axum::http::Request;
use std::task::{Context, Poll};
use tower::{Layer, Service, ServiceExt};

#[derive(Clone)]
struct MockService;

impl Service<Request<Body>> for MockService {
    type Response = axum::http::Response<Body>;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Body>) -> Self::Future {
        std::future::ready(Ok(axum::http::Response::new(Body::empty())))
    }
}

#[tokio::test]
async fn havoc_rate_limit_fuzz_bounds() {
    let cases = vec![
        (0.0, 0),
        (f64::NAN, 100),
        (f64::INFINITY, 10),
        (f64::NEG_INFINITY, 5),
        (-1.0, 5),
        (1e300, 100),
    ];

    for (rps, burst) in cases {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: rps,
            burst,
            trust_forwarded_headers: true,
        };

        let layer = RateLimitLayer::from_config(&config);
        let mut svc = layer.layer(MockService);

        let req = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4")
            .body(Body::empty())
            .unwrap();

        // Should not panic on first request
        let _ = svc.ready().await.unwrap().call(req).await;

        let req2 = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4")
            .body(Body::empty())
            .unwrap();

        // Should not panic on second request (where it might deny or hit edge cases)
        let _ = svc.ready().await.unwrap().call(req2).await;

        let req3 = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4")
            .body(Body::empty())
            .unwrap();

        // Should not panic on third request (ensures deficit calculation is fully evaluated)
        let _ = svc.ready().await.unwrap().call(req3).await;
    }

    // Force evaluate remaining_tokens branches
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: f64::MAX,
        burst: u32::MAX,
        trust_forwarded_headers: true,
    };
    let layer = RateLimitLayer::from_config(&config);
    let mut svc = layer.layer(MockService);
    let req = Request::builder()
        .header("X-Forwarded-For", "1.2.3.4")
        .body(Body::empty())
        .unwrap();
    let _ = svc.ready().await.unwrap().call(req).await;
}
