use autumn_web::security::RateLimitConfig;
use autumn_web::security::RateLimitLayer;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use std::net::SocketAddr;
use std::task::{Context, Poll};
use tower::{Layer, Service};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_rate_limit_concurrency_stress() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 1000.0,
        burst: 2000,
        trust_forwarded_headers: true,
        trusted_proxies: Vec::new(),
        ..Default::default()
    };
    let layer = RateLimitLayer::from_config(&config);
    let svc = layer.layer(MockService);

    let mut handles = vec![];
    for i in 0..1000 {
        let mut svc_clone = svc.clone();
        handles.push(tokio::spawn(async move {
            let mut req = Request::builder()
                .header("X-Forwarded-For", format!("10.0.0.{i}"))
                .body(Body::empty())
                .unwrap();
            let peer: SocketAddr = "198.51.100.1:2000".parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(peer));
            let _ = svc_clone.call(req).await;
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}
