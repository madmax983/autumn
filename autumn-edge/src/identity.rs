//! Normalized authenticated claims that may cross the edge wire.

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::request::Parts;
use serde::{Deserialize, Serialize};

use crate::wire::{FALLTHROUGH_SENTINEL, FallthroughReason};

/// Stable, normalized user identifier safe to send to a capsule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EdgeUserId(String);

impl EdgeUserId {
    /// Construct a normalized identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// Read the normalized identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A normalized authorization role safe to send to a capsule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EdgeRole(String);

impl EdgeRole {
    /// Construct a normalized role.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// Read the normalized role.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The complete identity envelope permitted to cross the edge wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeIdentity {
    user_id: EdgeUserId,
    roles: Vec<EdgeRole>,
}

impl EdgeIdentity {
    /// Construct an identity from normalized claims only.
    #[must_use]
    pub const fn new(user_id: EdgeUserId, roles: Vec<EdgeRole>) -> Self {
        Self { user_id, roles }
    }
    /// Authenticated user claim.
    #[must_use]
    pub const fn user_id(&self) -> &EdgeUserId {
        &self.user_id
    }
    /// Normalized role claims.
    #[must_use]
    pub fn roles(&self) -> &[EdgeRole] {
        &self.roles
    }
}

/// Rejection for a route requiring identity when the host supplied none.
#[derive(Clone, Copy, Debug)]
pub struct EdgeIdentityRequired;

impl IntoResponse for EdgeIdentityRequired {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            [(
                FALLTHROUGH_SENTINEL,
                FallthroughReason::MissingCapability.as_str(),
            )],
            "edge identity required",
        )
            .into_response()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for EdgeIdentity {
    type Rejection = EdgeIdentityRequired;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(EdgeIdentityRequired)
    }
}

/// Origin-side gate for `#[edge(needs(identity))]` routes (#3086).
///
/// The capsule declines an unauthenticated request *before* dispatch
/// (fallthrough on [`FallthroughReason::MissingCapability`]), so without this
/// the native mount answers requests the edge lane would never serve: a
/// declaration-only identity route stays reachable through the origin
/// fallback. The route macro applies this to the native mount of every
/// handler that declares `needs(identity)`; a request whose extensions carry
/// a host-resolved [`EdgeIdentity`] runs on, anything else gets the same 401
/// plus [`FALLTHROUGH_SENTINEL`] rejection the extractor returns, so edge and
/// origin agree on the outcome.
///
/// Use with `axum::middleware::from_fn`. Route-local by construction, like
/// [`crate::strip_request_credentials`]: the app's own middleware wraps the
/// router above this layer and still sees the original request.
pub async fn require_edge_identity(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.extensions().get::<EdgeIdentity>().is_some() {
        next.run(request).await
    } else {
        EdgeIdentityRequired.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt as _;

    fn gated_app() -> axum::Router {
        axum::Router::new()
            .route("/", get(|| async { "served" }))
            .layer(axum::middleware::from_fn(require_edge_identity))
    }

    fn get_request() -> axum::extract::Request {
        http::Request::builder()
            .uri("/")
            .body(Body::from(""))
            .expect("a bare GET request builds")
    }

    #[test]
    fn anonymous_requests_are_rejected_with_the_fallthrough_sentinel() {
        let response = futures::executor::block_on(gated_app().oneshot(get_request()))
            .expect("the service responds");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let sentinel = response
            .headers()
            .get(FALLTHROUGH_SENTINEL)
            .expect("the rejection carries the fallthrough sentinel");
        assert_eq!(
            sentinel.as_bytes(),
            FallthroughReason::MissingCapability.as_str().as_bytes(),
            "the sentinel names the missing capability, like the extractor's rejection"
        );
    }

    #[test]
    fn requests_carrying_a_host_resolved_identity_are_forwarded() {
        let mut request = get_request();
        request.extensions_mut().insert(EdgeIdentity::new(
            EdgeUserId::new("user-1"),
            vec![EdgeRole::new("reader")],
        ));

        let response = futures::executor::block_on(gated_app().oneshot(request))
            .expect("the service responds");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
