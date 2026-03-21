//! Framework error type and result alias.
//!
//! [`AutumnError`] wraps any `Error + Send + Sync` with an HTTP status code.
//! The blanket [`From`] impl maps all errors to 500 Internal Server Error,
//! so the `?` operator works in handlers with zero ceremony.
//!
//! For non-500 cases, use the status refinement methods:
//! [`not_found()`](AutumnError::not_found),
//! [`bad_request()`](AutumnError::bad_request),
//! [`unprocessable()`](AutumnError::unprocessable), or
//! [`with_status()`](AutumnError::with_status).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Framework error type wrapping any error with an HTTP status code.
///
/// # Usage
///
/// The `?` operator converts any `Error` into an `AutumnError` with status 500:
/// ```ignore
/// async fn handler() -> AutumnResult<&'static str> {
///     might_fail()?; // becomes 500 on error
///     Ok("ok")
/// }
/// ```
///
/// For expected errors, use status refinement:
/// ```ignore
/// async fn get_user(id: Path<i32>) -> AutumnResult<Json<User>> {
///     let user = find_user(id.0)
///         .map_err(AutumnError::not_found)?; // 404
///     Ok(Json(user))
/// }
/// ```
///
/// # Why no `Error` impl
///
/// `AutumnError` intentionally does **not** implement [`std::error::Error`].
/// Doing so would conflict with the blanket `From<E: Error>` impl (the
/// reflexive `From<T> for T` would overlap). This type is a *response* type,
/// not a propagatable error.
pub struct AutumnError {
    inner: Box<dyn std::error::Error + Send + Sync>,
    status: StatusCode,
}

/// Convenience alias — the standard return type for Autumn handlers.
pub type AutumnResult<T> = Result<T, AutumnError>;

impl<E> From<E> for AutumnError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl AutumnError {
    /// Override the HTTP status code.
    #[must_use]
    pub const fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Create a 404 Not Found error.
    pub fn not_found(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::NOT_FOUND,
        }
    }

    /// Create a 400 Bad Request error.
    pub fn bad_request(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::BAD_REQUEST,
        }
    }

    /// Create a 422 Unprocessable Entity error.
    pub fn unprocessable(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    /// Returns the HTTP status code.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }
}

impl std::fmt::Display for AutumnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::fmt::Debug for AutumnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutumnError")
            .field("status", &self.status)
            .field("inner", &self.inner)
            .finish()
    }
}

impl IntoResponse for AutumnError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = serde_json::json!({
            "error": {
                "status": status.as_u16(),
                "message": self.inner.to_string(),
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestError {}

    #[test]
    fn blanket_from_defaults_to_500() {
        let err: AutumnError = TestError("boom".into()).into();
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn not_found_is_404() {
        let err = AutumnError::not_found(TestError("missing".into()));
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_request_is_400() {
        let err = AutumnError::bad_request(TestError("invalid input".into()));
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unprocessable_is_422() {
        let err = AutumnError::unprocessable(TestError("bad entity".into()));
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn with_status_overrides() {
        let err: AutumnError = TestError("forbidden".into()).into();
        let err = err.with_status(StatusCode::FORBIDDEN);
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn display_uses_inner_message() {
        let err: AutumnError = TestError("something broke".into()).into();
        assert_eq!(err.to_string(), "something broke");
    }

    #[test]
    fn into_response_has_correct_status() {
        let err = AutumnError::not_found(TestError("not found".into()));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn into_response_has_json_body() {
        let err = AutumnError::not_found(TestError("not found".into()));
        let response = err.into_response();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["error"]["status"], 404);
        assert_eq!(json["error"]["message"], "not found");
    }
}
