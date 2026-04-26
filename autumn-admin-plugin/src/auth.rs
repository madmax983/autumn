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
/// Returns 401 if `auth_session_key` is absent from the session, 403 if
/// the role doesn't match. The session key matches Autumn's
/// `auth.session_key` config (default `"user_id"`); deployments that
/// changed it (e.g. to `"uid"`) must pass the same string here via
/// [`crate::AdminPlugin::auth_session_key`].
///
/// Errors are produced via `AutumnError::*_msg` so the framework's
/// error-page filter renders them as branded HTML for browser clients
/// and JSON for API clients.
pub async fn check_role(
    role: String,
    auth_session_key: String,
    req: Request,
    next: Next,
) -> Response {
    let Some(session) = req.extensions().get::<Session>().cloned() else {
        return AutumnError::internal_server_error_msg(
            "SessionLayer not installed; admin plugin requires sessions",
        )
        .into_response();
    };

    if session.get(&auth_session_key).await.is_none() {
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
        app_with_role_gate_and_key(session, "user_id")
    }

    fn app_with_role_gate_and_key(session: Session, auth_session_key: &'static str) -> Router {
        async fn ok() -> &'static str {
            "ok"
        }
        let role = "admin".to_owned();
        let key = auth_session_key.to_owned();
        Router::new()
            .route("/", get(ok))
            .layer(from_fn(move |mut req: Request, next: Next| {
                let session = session.clone();
                let role = role.clone();
                let key = key.clone();
                async move {
                    req.extensions_mut().insert(session);
                    check_role(role, key, req, next).await
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
    async fn custom_auth_session_key_is_honored() {
        // Deployment configured `auth.session_key = "uid"`. A session with
        // "uid" populated must authenticate; a session with only the
        // default "user_id" must NOT (because the deployment's auth
        // pipeline never writes "user_id").
        let with_uid = Session::new_for_test(
            "sid".into(),
            HashMap::from([("uid".into(), "42".into()), ("role".into(), "admin".into())]),
        );
        let app = app_with_role_gate_and_key(with_uid, "uid");
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "uid-keyed session should pass"
        );

        // Inverse: only the default "user_id" present, but the deployment
        // is configured for "uid" — must reject as unauthenticated.
        let with_user_id = Session::new_for_test(
            "sid".into(),
            HashMap::from([
                ("user_id".into(), "42".into()),
                ("role".into(), "admin".into()),
            ]),
        );
        let app = app_with_role_gate_and_key(with_user_id, "uid");
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "wrong-key session must NOT authenticate"
        );
    }

    #[tokio::test]
    async fn missing_session_extension_returns_500() {
        async fn ok() -> &'static str {
            "ok"
        }
        let role = "admin".to_owned();
        let key = "user_id".to_owned();
        let app =
            Router::new()
                .route("/", get(ok))
                .layer(from_fn(move |req: Request, next: Next| {
                    let role = role.clone();
                    let key = key.clone();
                    async move { check_role(role, key, req, next).await }
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
