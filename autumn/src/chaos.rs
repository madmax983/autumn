//! Chaos engineering middleware and plugin for Autumn.
//!
//! Provides a Tower middleware that injects artificial latency and HTTP 500
//! errors to test frontend resilience and retry logic. To prevent accidental
//! disruption in production, chaos is only injected when the `X-Chaos-Token`
//! request header matches the configured secret token.
//!
//! # Example
//!
//! ```rust,ignore
//! use autumn_web::chaos::{ChaosPlugin, ChaosConfig};
//!
//! autumn_web::app()
//!     .plugin(ChaosPlugin::new(ChaosConfig {
//!         enabled: true,
//!         token: "super-secret-chaos-token".to_string(),
//!         max_latency_ms: 2000,
//!         error_rate: 25, // 25% chance of 500 error
//!     }))
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderValue, Request, Response, StatusCode};
use futures::future::BoxFuture;
use tower::{Layer, Service};

use crate::app::AppBuilder;
use crate::plugin::Plugin;

/// Configuration for chaos engineering injection.
#[derive(Debug, Clone, Default)]
pub struct ChaosConfig {
    /// Whether chaos injection is active.
    pub enabled: bool,
    /// The secret token that must be provided in the `X-Chaos-Token` header.
    pub token: String,
    /// The maximum artificial latency to inject, in milliseconds.
    pub max_latency_ms: u64,
    /// The percentage chance (0-100) of returning a 500 Internal Server Error.
    pub error_rate: u8,
}

/// A Tower layer that applies chaos engineering effects.
#[derive(Clone)]
pub struct ChaosLayer {
    config: Arc<ChaosConfig>,
}

impl ChaosLayer {
    /// Create a new chaos layer with the given configuration.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(config: ChaosConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl<S> Layer<S> for ChaosLayer {
    type Service = ChaosService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ChaosService {
            inner,
            config: self.config.clone(),
        }
    }
}

/// A Tower service that injects chaos engineering effects.
#[derive(Clone)]
pub struct ChaosService<S> {
    inner: S,
    config: Arc<ChaosConfig>,
}

// Minimal pseudorandom number generator since we don't want to pull in `rand`
// if we don't strictly need it, and `SystemTime` hashing is fast enough for chaos testing.
fn random_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    #[allow(clippy::cast_possible_truncation)]
    let mut hash = now.as_nanos() as u64;
    hash = hash.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed));
    // Simple xorshift
    hash ^= hash << 13;
    hash ^= hash >> 7;
    hash ^= hash << 17;
    hash
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for ChaosService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + Sync + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let config = self.config.clone();

        let inject_chaos = if config.enabled {
            req.headers()
                .get("X-Chaos-Token")
                .map(HeaderValue::as_bytes)
                == Some(config.token.as_bytes())
        } else {
            false
        };

        if !inject_chaos {
            let clone = self.inner.clone();
            let mut inner = std::mem::replace(&mut self.inner, clone);
            return Box::pin(async move { inner.call(req).await });
        }

        // We know we are injecting chaos. Swap the service.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let r = random_u64();

            // 1. Determine if we should fail
            if config.error_rate > 0 {
                let fail_threshold = u64::from(config.error_rate).min(100);
                if (r % 100) < fail_threshold {
                    let mut res = Response::new(ResBody::default());
                    *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    res.headers_mut()
                        .insert("X-Chaos-Injected", HeaderValue::from_static("error"));
                    return Ok(res);
                }
            }

            // 2. Inject latency
            if config.max_latency_ms > 0 {
                let r2 = random_u64();
                let latency = r2 % config.max_latency_ms;
                tokio::time::sleep(Duration::from_millis(latency)).await;
            }

            let mut res = inner.call(req).await?;
            res.headers_mut()
                .insert("X-Chaos-Injected", HeaderValue::from_static("latency"));
            Ok(res)
        })
    }
}

/// Plugin that adds the `ChaosLayer` to the application.
pub struct ChaosPlugin {
    config: ChaosConfig,
}

impl ChaosPlugin {
    /// Create a new chaos plugin with the given configuration.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(config: ChaosConfig) -> Self {
        Self { config }
    }
}

impl Plugin for ChaosPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        app.layer(ChaosLayer::new(self.config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use std::time::Instant;
    use tower::ServiceExt;

    fn test_router(config: ChaosConfig) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(ChaosLayer::new(config))
    }

    #[tokio::test]
    async fn disabled_config_bypasses_chaos() {
        let app = test_router(ChaosConfig {
            enabled: false,
            token: "secret".to_string(),
            max_latency_ms: 1000,
            error_rate: 100, // Would always fail if enabled
        });

        let req = Request::builder()
            .uri("/")
            .header("X-Chaos-Token", "secret")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!res.headers().contains_key("X-Chaos-Injected"));
    }

    #[tokio::test]
    async fn missing_or_invalid_token_bypasses_chaos() {
        let app = test_router(ChaosConfig {
            enabled: true,
            token: "secret".to_string(),
            max_latency_ms: 1000,
            error_rate: 100, // Would always fail if token matched
        });

        // No token
        let req1 = Request::builder().uri("/").body(Body::empty()).unwrap();
        let result_no_token = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(result_no_token.status(), StatusCode::OK);
        assert!(!result_no_token.headers().contains_key("X-Chaos-Injected"));

        // Invalid token
        let req2 = Request::builder()
            .uri("/")
            .header("X-Chaos-Token", "wrong")
            .body(Body::empty())
            .unwrap();
        let result_invalid_token = app.oneshot(req2).await.unwrap();
        assert_eq!(result_invalid_token.status(), StatusCode::OK);
        assert!(
            !result_invalid_token
                .headers()
                .contains_key("X-Chaos-Injected")
        );
    }

    #[tokio::test]
    async fn valid_token_injects_error() {
        let app = test_router(ChaosConfig {
            enabled: true,
            token: "secret".to_string(),
            max_latency_ms: 0,
            error_rate: 100, // 100% chance of error
        });

        let req = Request::builder()
            .uri("/")
            .header("X-Chaos-Token", "secret")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(res.headers().get("X-Chaos-Injected").unwrap(), "error");
    }

    #[tokio::test]
    async fn valid_token_injects_latency() {
        let app = test_router(ChaosConfig {
            enabled: true,
            token: "secret".to_string(),
            max_latency_ms: 100, // Up to 100ms
            error_rate: 0,       // Never fail
        });

        let req = Request::builder()
            .uri("/")
            .header("X-Chaos-Token", "secret")
            .body(Body::empty())
            .unwrap();

        let start = Instant::now();
        let res = app.oneshot(req).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("X-Chaos-Injected").unwrap(), "latency");
        // It could be very fast if random latency was near 0, but it should succeed.
        // We mainly test that it compiles, executes, and sets the header.
        assert!(elapsed < Duration::from_millis(150));
    }
}
