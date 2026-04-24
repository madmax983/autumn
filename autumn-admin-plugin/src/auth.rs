//! Role-check middleware for the admin router.
//!
//! Wraps the nested admin router with a `from_fn` layer that inspects the
//! request's [`Session`] and short-circuits with [`AutumnError::unauthorized_msg`]
//! or [`AutumnError::forbidden_msg`] when the required role is missing.

use autumn_web::AutumnError;
use autumn_web::session::Session;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Produce an axum middleware that verifies the incoming request has a
/// logged-in session with the given role.
///
/// Returns 401 if no `user_id` is in the session, 403 if the role doesn't
/// match. Errors are produced via `AutumnError::*_msg` so the framework's
/// error-page filter renders them as branded HTML for browser clients and
/// JSON for API clients.
pub async fn check_role(role: String, req: Request, next: Next) -> Response {
    let Some(session) = req.extensions().get::<Session>().cloned() else {
        return AutumnError::internal_server_error_msg(
            "SessionLayer not installed; admin plugin requires sessions",
        )
        .into_response();
    };

    if session.get("user_id").await.is_none() {
        return AutumnError::unauthorized_msg("authentication required").into_response();
    }
    let current = session.get("role").await.unwrap_or_default();
    if current != role {
        return AutumnError::forbidden_msg(format!("'{role}' role required")).into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::session::Session;
    use axum::Router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware::from_fn;
    use axum::routing::get;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn app_with_role_gate(session: Session) -> Router {
        async fn ok() -> &'static str {
            "ok"
        }
        let role = "admin".to_owned();
        Router::new()
            .route("/", get(ok))
            .layer(from_fn(move |mut req: Request, next: Next| {
                let session = session.clone();
                let role = role.clone();
                async move {
                    req.extensions_mut().insert(session);
                    check_role(role, req, next).await
                }
            }))
    }

    #[tokio::test]
    async fn no_session_returns_401() {
        let session = Session::new_for_test("sid".into(), HashMap::new());
        let app = app_with_role_gate(session);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_role_returns_403() {
        let session = Session::new_for_test(
            "sid".into(),
            HashMap::from([
                ("user_id".into(), "1".into()),
                ("role".into(), "viewer".into()),
            ]),
        );
        let app = app_with_role_gate(session);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn correct_role_passes_through() {
        let session = Session::new_for_test(
            "sid".into(),
            HashMap::from([
                ("user_id".into(), "1".into()),
                ("role".into(), "admin".into()),
            ]),
        );
        let app = app_with_role_gate(session);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_session_extension_returns_500() {
        async fn ok() -> &'static str {
            "ok"
        }
        let role = "admin".to_owned();
        let app =
            Router::new()
                .route("/", get(ok))
                .layer(from_fn(move |req: Request, next: Next| {
                    let role = role.clone();
                    async move { check_role(role, req, next).await }
                }));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
