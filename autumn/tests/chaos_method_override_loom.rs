use autumn_web::middleware::MethodOverrideLayer;
use axum::body::Body;
use axum::http::{Method, Request};
use loom::thread;
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

#[test]
fn method_override_concurrent() {
    loom::model(|| {
        let layer = MethodOverrideLayer::new();
        let svc = layer.layer(MockService);

        let mut s1 = svc.clone();
        let t1 = thread::spawn(move || {
            let req = Request::builder()
                .method(Method::POST)
                .header("X-HTTP-Method-Override", "PUT")
                .body(Body::empty())
                .unwrap();
            let _ = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(s1.call(req));
        });

        let mut s2 = svc;
        let t2 = thread::spawn(move || {
            let req = Request::builder()
                .method(Method::POST)
                .header("X-HTTP-Method-Override", "DELETE")
                .body(Body::empty())
                .unwrap();
            let _ = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(s2.call(req));
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}
