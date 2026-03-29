use autumn_web::get;

/// A trivial Tower layer that passes through unchanged.
#[derive(Clone)]
struct PassthroughLayer;

impl<S> tower::Layer<S> for PassthroughLayer {
    type Service = S;
    fn layer(&self, inner: S) -> Self::Service {
        inner
    }
}

#[get("/hello")]
#[intercept(PassthroughLayer)]
async fn hello() -> &'static str {
    "hello"
}

#[get("/multi")]
#[intercept(PassthroughLayer)]
#[intercept(PassthroughLayer)]
async fn multi() -> &'static str {
    "multi"
}

fn main() {}
