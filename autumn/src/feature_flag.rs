//! Feature flags for progressive delivery and toggles.
//!
//! Provides a [`FeatureFlagStore`] trait, an [`InMemoryFeatureFlagStore`] implementation,
//! a [`FeatureFlag`] extractor, and a [`RequireFeature`] middleware.
//!
//! Feature flags allow you to decouple deployment from release by gating features
//! behind runtime checks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;

/// A trait for retrieving and managing feature flags at runtime.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// concurrent request handlers via the Axum state or extensions.
pub trait FeatureFlagStore: Send + Sync + 'static {
    /// Returns `true` if the specified feature is currently enabled.
    fn is_enabled(&self, feature: &str) -> bool;

    /// Enables the specified feature.
    fn enable(&self, feature: &str);

    /// Disables the specified feature.
    fn disable(&self, feature: &str);
}

/// A simple in-memory feature flag store.
///
/// This store is primarily useful for development, testing, or small single-node
/// applications. For distributed applications, implement [`FeatureFlagStore`]
/// over Redis or a database.
#[derive(Debug, Default)]
pub struct InMemoryFeatureFlagStore {
    flags: RwLock<HashMap<String, bool>>,
}

impl InMemoryFeatureFlagStore {
    /// Creates a new, empty in-memory feature flag store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl FeatureFlagStore for InMemoryFeatureFlagStore {
    fn is_enabled(&self, feature: &str) -> bool {
        self.flags
            .read()
            .unwrap()
            .get(feature)
            .copied()
            .unwrap_or(false)
    }

    fn enable(&self, feature: &str) {
        self.flags
            .write()
            .unwrap()
            .insert(feature.to_string(), true);
    }

    fn disable(&self, feature: &str) {
        self.flags
            .write()
            .unwrap()
            .insert(feature.to_string(), false);
    }
}

/// An extractor for checking feature flags in Axum route handlers.
///
/// Requires an `Arc<dyn FeatureFlagStore>` to be present in the request
/// extensions. This is typically added globally using `axum::Extension`.
///
/// # Examples
///
/// ```rust,ignore
/// use autumn_web::feature_flag::FeatureFlag;
///
/// async fn my_handler(flags: FeatureFlag) -> &'static str {
///     if flags.is_enabled("new_dashboard") {
///         "Welcome to the new dashboard!"
///     } else {
///         "Welcome to the classic dashboard!"
///     }
/// }
/// ```
#[derive(Clone)]
pub struct FeatureFlag {
    store: Arc<dyn FeatureFlagStore>,
}

impl FeatureFlag {
    /// Returns `true` if the specified feature is enabled.
    #[must_use]
    pub fn is_enabled(&self, feature: &str) -> bool {
        self.store.is_enabled(feature)
    }
}

impl<S> FromRequestParts<S> for FeatureFlag
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let store = parts.extensions.get::<Arc<dyn FeatureFlagStore>>().ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "FeatureFlagStore missing from request extensions",
        ))?;

        Ok(Self {
            store: Arc::clone(store),
        })
    }
}

/// A middleware layer that restricts access to routes based on a feature flag.
///
/// Returns `404 Not Found` if the specified feature is disabled. This is
/// preferable to `403 Forbidden` for most feature flags because it hides the
/// existence of the disabled endpoint.
///
/// Requires an `Arc<dyn FeatureFlagStore>` to be present in the request extensions.
#[derive(Clone)]
pub struct RequireFeature {
    feature: String,
}

impl RequireFeature {
    /// Creates a new `RequireFeature` middleware for the specified feature name.
    #[must_use]
    pub fn new(feature: impl Into<String>) -> Self {
        Self {
            feature: feature.into(),
        }
    }
}

impl<S> tower::Layer<S> for RequireFeature {
    type Service = RequireFeatureMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequireFeatureMiddleware {
            inner,
            feature: self.feature.clone(),
        }
    }
}

/// The inner service for [`RequireFeature`].
#[derive(Clone)]
pub struct RequireFeatureMiddleware<S> {
    inner: S,
    feature: String,
}

impl<S> tower::Service<Request> for RequireFeatureMiddleware<S>
where
    S: tower::Service<Request, Response = Response> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let feature = self.feature.clone();

        let store_is_enabled = req
            .extensions()
            .get::<Arc<dyn FeatureFlagStore>>()
            .is_some_and(|store| store.is_enabled(&feature));

        if store_is_enabled {
            let clone = self.inner.clone();
            let mut inner = std::mem::replace(&mut self.inner, clone);
            Box::pin(async move { inner.call(req).await })
        } else {
            // Feature disabled: return 404 Not Found
            Box::pin(async move { Ok(StatusCode::NOT_FOUND.into_response()) })
        }
    }
}

/// Convenience function for attaching the `RequireFeature` middleware.
///
/// # Examples
///
/// ```rust,ignore
/// use axum::{Router, routing::get, middleware};
/// use autumn_web::feature_flag::require_feature;
///
/// let app = Router::new()
///     .route("/beta", get(|| async { "Beta feature" }))
///     .route_layer(require_feature("beta"));
/// ```
#[must_use]
pub fn require_feature(feature: impl Into<String>) -> RequireFeature {
    RequireFeature::new(feature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Extension;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    #[test]
    fn test_in_memory_store() {
        let store = InMemoryFeatureFlagStore::new();

        assert!(!store.is_enabled("test_feature"));

        store.enable("test_feature");
        assert!(store.is_enabled("test_feature"));

        store.disable("test_feature");
        assert!(!store.is_enabled("test_feature"));
    }

    #[tokio::test]
    async fn test_feature_flag_extractor() {
        let store = InMemoryFeatureFlagStore::new();
        store.enable("extractor_test");

        let app = Router::new()
            .route(
                "/",
                get(|flags: FeatureFlag| async move {
                    if flags.is_enabled("extractor_test") {
                        "enabled"
                    } else {
                        "disabled"
                    }
                }),
            )
            .layer(Extension(Arc::new(store) as Arc<dyn FeatureFlagStore>));

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"enabled");
    }

    #[tokio::test]
    async fn test_require_feature_middleware_enabled() {
        let store = InMemoryFeatureFlagStore::new();
        store.enable("middleware_test");

        let app = Router::new()
            .route("/protected", get(|| async { "Success" }))
            .route_layer(require_feature("middleware_test"))
            .layer(Extension(Arc::new(store) as Arc<dyn FeatureFlagStore>));

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_require_feature_middleware_disabled() {
        let store = InMemoryFeatureFlagStore::new();
        // middleware_test is disabled by default

        let app = Router::new()
            .route("/protected", get(|| async { "Success" }))
            .route_layer(require_feature("middleware_test"))
            .layer(Extension(Arc::new(store) as Arc<dyn FeatureFlagStore>));

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should return 404
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
