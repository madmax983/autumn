//! Tower middleware that caches HTTP GET responses.
//!
//! Only caches `GET` requests that produce `200 OK` responses.
//! Non-GET methods and non-200 responses pass through untouched.
//!
//! # Usage
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::cache::{CacheResponseLayer, MokaCache};
//!
//! let store = MokaCache::builder()
//!     .max_capacity(1000)
//!     .ttl(std::time::Duration::from_secs(300))
//!     .build();
//!
//! #[get("/users/{id}")]
//! #[intercept(CacheResponseLayer::from_cache(store))]
//! async fn get_user(Path(id): Path<i32>) -> Json<User> { ... }
//! ```

use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Method, StatusCode};
use http::Request;
use http_body_util::BodyExt;
use tower::{Layer, Service};

use super::Cache;

/// A cached HTTP response: status, headers, and body bytes.
#[derive(Clone)]
struct CachedResponse {
    status: StatusCode,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

/// Tower layer that caches HTTP GET responses.
///
/// Wrap around a handler via `#[intercept(CacheResponseLayer::from_cache(store))]`
/// or construct manually and apply with `.layer()`.
///
/// Caching rules:
/// - Only `GET` requests are cached.
/// - Only `200 OK` responses are cached.
/// - The cache key is the request URI path + query string.
#[derive(Clone)]
pub struct CacheResponseLayer {
    store: Arc<dyn Cache>,
}

impl CacheResponseLayer {
    /// Create a layer backed by the given cache store.
    pub fn from_cache(store: impl Cache + 'static) -> Self {
        Self {
            store: Arc::new(store),
        }
    }

    /// Create from an existing `Arc<dyn Cache>`.
    pub fn from_shared(store: Arc<dyn Cache>) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for CacheResponseLayer {
    type Service = CacheResponseService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CacheResponseService {
            inner,
            store: self.store.clone(),
        }
    }
}

/// The [`Service`] produced by [`CacheResponseLayer`].
#[derive(Clone)]
pub struct CacheResponseService<S> {
    inner: S,
    store: Arc<dyn Cache>,
}

impl<S> Service<Request<Body>> for CacheResponseService<S>
where
    S: Service<Request<Body>, Response = axum::response::Response, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // Only cache GET requests
        if req.method() != Method::GET {
            return Box::pin(self.inner.call(req));
        }

        let cache_key = format!("http:{}", req.uri());
        let store = self.store.clone();

        // Check for a cache hit
        if let Some(cached) = super::get::<CachedResponse>(store.as_ref(), &cache_key) {
            return Box::pin(async move {
                let mut builder = axum::response::Response::builder().status(cached.status);
                if let Some(headers) = builder.headers_mut() {
                    headers.extend(cached.headers);
                }
                let resp = builder.body(Body::from(cached.body)).unwrap_or_else(|_| {
                    axum::response::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::empty())
                        .expect("test requirement failed")
                });
                Ok(resp)
            });
        }

        // Cache miss — call the inner service
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let response = inner.call(req).await?;

            // Only cache 200 OK responses
            if response.status() != StatusCode::OK {
                return Ok(response);
            }

            let (parts, body) = response.into_parts();

            // Buffer the body
            let Ok(collected) = body.collect().await else {
                let resp = axum::response::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .expect("test requirement failed");
                return Ok(resp);
            };
            let body_bytes = collected.to_bytes();

            // Store in cache
            let cached = CachedResponse {
                status: parts.status,
                headers: parts.headers.clone(),
                body: body_bytes.clone(),
            };
            super::insert(store.as_ref(), &cache_key, cached);

            // Reconstruct the response
            let response = axum::response::Response::from_parts(parts, Body::from(body_bytes));
            Ok(response)
        })
    }
}

#[cfg(all(test, feature = "cache-moka"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::{ServiceBuilder, ServiceExt};

    /// Build a test service that returns a fixed body and counts calls.
    fn counting_service(
        counter: Arc<AtomicUsize>,
        body: &'static str,
    ) -> impl Service<
        Request<Body>,
        Response = axum::response::Response,
        Error = Infallible,
        Future = impl std::future::Future<Output = Result<axum::response::Response, Infallible>> + Send,
    > + Clone
    + Send
    + 'static {
        let body = body.to_owned();
        tower::service_fn(move |_req: Request<Body>| {
            let counter = counter.clone();
            let body = body.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(body))
                    .expect("test requirement failed"))
            }
        })
    }

    #[tokio::test]
    async fn caches_get_responses() {
        let store = super::super::MokaCache::new(100, None);
        let counter = Arc::new(AtomicUsize::new(0));

        let mut svc = ServiceBuilder::new()
            .layer(CacheResponseLayer::from_cache(store))
            .service(counting_service(counter.clone(), "hello"));

        // First request — cache miss
        let req = Request::get("/test")
            .body(Body::empty())
            .expect("test requirement failed");
        let resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .expect("test requirement failed")
            .to_bytes();
        assert_eq!(body.as_ref(), b"hello");
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Second request — cache hit, inner service NOT called
        let req = Request::get("/test")
            .body(Body::empty())
            .expect("test requirement failed");
        let resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .expect("test requirement failed")
            .to_bytes();
        assert_eq!(body.as_ref(), b"hello");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "inner should not be called again"
        );
    }

    #[tokio::test]
    async fn does_not_cache_post_requests() {
        let store = super::super::MokaCache::new(100, None);
        let counter = Arc::new(AtomicUsize::new(0));

        let mut svc = ServiceBuilder::new()
            .layer(CacheResponseLayer::from_cache(store))
            .service(counting_service(counter.clone(), "created"));

        let req = Request::post("/items")
            .body(Body::empty())
            .expect("test requirement failed");
        let _resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        let req = Request::post("/items")
            .body(Body::empty())
            .expect("test requirement failed");
        let _resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "POST should not be cached"
        );
    }

    #[tokio::test]
    async fn does_not_cache_non_200_responses() {
        let store = super::super::MokaCache::new(100, None);
        let counter = Arc::new(AtomicUsize::new(0));

        let svc_inner = {
            let counter = counter.clone();
            tower::service_fn(move |_req: Request<Body>| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Infallible>(
                        axum::response::Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::from("not found"))
                            .expect("test requirement failed"),
                    )
                }
            })
        };

        let mut svc = ServiceBuilder::new()
            .layer(CacheResponseLayer::from_cache(store))
            .service(svc_inner);

        let req = Request::get("/missing")
            .body(Body::empty())
            .expect("test requirement failed");
        let resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let req = Request::get("/missing")
            .body(Body::empty())
            .expect("test requirement failed");
        let resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "404 should not be cached"
        );
    }

    #[tokio::test]
    async fn different_uris_cached_separately() {
        let store = super::super::MokaCache::new(100, None);
        let counter = Arc::new(AtomicUsize::new(0));

        let mut svc = ServiceBuilder::new()
            .layer(CacheResponseLayer::from_cache(store))
            .service(counting_service(counter.clone(), "ok"));

        let req = Request::get("/a")
            .body(Body::empty())
            .expect("test requirement failed");
        let _resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        let req = Request::get("/b")
            .body(Body::empty())
            .expect("test requirement failed");
        let _resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "different URIs should miss"
        );

        // But repeating /a should hit
        let req = Request::get("/a")
            .body(Body::empty())
            .expect("test requirement failed");
        let _resp = svc
            .ready()
            .await
            .expect("test requirement failed")
            .call(req)
            .await
            .expect("test requirement failed");
        assert_eq!(counter.load(Ordering::SeqCst), 2, "/a should be cached");
    }

    #[test]
    fn from_shared_accepts_arc() {
        let store = Arc::new(super::super::MokaCache::new(100, None));
        // Just verify from_shared compiles and the layer can be used
        let _layer = CacheResponseLayer::from_shared(store);
    }
}
