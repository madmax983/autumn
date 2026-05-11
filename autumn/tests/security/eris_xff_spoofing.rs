use autumn_web::security::RateLimitConfig;
use autumn_web::security::RateLimitLayer;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use std::net::SocketAddr;
use tower::{Layer, Service};

#[derive(Clone)]
struct MockService;

impl Service<Request<Body>> for MockService {
    type Response = axum::http::Response<Body>;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Body>) -> Self::Future {
        std::future::ready(Ok(axum::http::Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap()))
    }
}

#[tokio::test]
async fn test_xff_spoofing() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 1.0,
        burst: 1,
        trust_forwarded_headers: true,
        trusted_proxies: vec![],
    };

    let layer = RateLimitLayer::from_config(&config);
    let mut app = layer.layer(MockService);

    let peer: SocketAddr = "198.51.100.1:2000".parse().unwrap();

    let make_req = |spoofed_ip: &str| {
        let mut req = Request::builder()
            .method("GET")
            .uri("/")
            .header("X-Forwarded-For", spoofed_ip)
            .header("X-Forwarded-For", "1.2.3.4") // Proxy appends the real IP in a new header
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        req
    };

    let res1 = app.call(make_req("spoofed_1")).await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    let res2 = app.call(make_req("spoofed_2")).await.unwrap();
    assert_eq!(
        res2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "Rate limit bypassed!"
    );
}
