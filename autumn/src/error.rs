//! Framework error type and result alias.
//!
//! [`AutumnError`] wraps any `Error + Send + Sync` with an HTTP status code.
//! The blanket [`From`] impl maps all errors to `500 Internal Server Error`,
//! so the `?` operator works in handlers with zero ceremony.
//!
//! For non-500 cases, use the status refinement constructors:
//!
//! - [`AutumnError::not_found`] -- 404
//! - [`AutumnError::bad_request`] -- 400
//! - [`AutumnError::unprocessable`] -- 422
//! - [`AutumnError::service_unavailable`] -- 503
//! - [`AutumnError::with_status`] -- arbitrary status code
//!
//! For simple string messages without wrapping an error type:
//!
//! - [`AutumnError::not_found_msg`] -- 404 with a message
//! - [`AutumnError::bad_request_msg`] -- 400 with a message
//! - [`AutumnError::unprocessable_msg`] -- 422 with a message
//! - [`AutumnError::service_unavailable_msg`] -- 503 with a message
//!
//! # Response format
//!
//! When an `AutumnError` is returned from a handler, it renders as JSON:
//!
//! ```json
//! { "error": { "status": 404, "message": "user not found" } }
//! ```
//!
//! # Examples
//!
//! ```rust
//! use autumn_web::error::AutumnError;
//! use http::StatusCode;
//!
//! // Blanket From impl: any Error becomes 500
//! let err: AutumnError = std::io::Error::other("disk full").into();
//! assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
//!
//! // Explicit status constructors
//! let err = AutumnError::not_found(std::io::Error::other("no such user"));
//! assert_eq!(err.status(), StatusCode::NOT_FOUND);
//! ```

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Simple error type wrapping a string message.
///
/// Used by the `_msg` convenience constructors on [`AutumnError`] so callers
/// don't need to wrap strings in `std::io::Error`.
#[derive(Debug)]
struct StringError(String);

impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StringError {}

/// Typed JSON body for error responses -- avoids dynamic `serde_json::Value`.
#[derive(Serialize)]
struct ErrorBody {
    error: ErrorInner,
}

#[derive(Serialize)]
struct ErrorInner {
    status: u16,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<std::collections::HashMap<String, Vec<String>>>,
}

/// JSON body for RFC 7807 Problem Details responses.
#[derive(Serialize)]
struct ProblemBody {
    #[serde(rename = "type")]
    type_uri: &'static str,
    title: &'static str,
    status: u16,
    detail: String,
}

/// Framework error type wrapping any error with an HTTP status code.
///
/// # Usage
///
/// The `?` operator converts any `std::error::Error` into an `AutumnError`
/// with status `500 Internal Server Error`:
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn handler() -> AutumnResult<&'static str> {
///     autumn_web::reexports::tokio::fs::read_to_string("missing.txt").await?; // becomes 500 on error
///     Ok("ok")
/// }
/// ```
///
/// For expected errors, use a status refinement constructor:
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/users/{id}")]
/// async fn get_user(axum::extract::Path(id): axum::extract::Path<i32>) -> AutumnResult<String> {
///     if id < 0 {
///         return Err(AutumnError::bad_request(
///             std::io::Error::other("id must be positive"),
///         ));
///     }
///     Ok(format!("user {id}"))
/// }
/// ```
///
/// # Why no `Error` impl
///
/// `AutumnError` intentionally does **not** implement [`std::error::Error`].
/// Doing so would conflict with the blanket `From<E: Error>` impl (the
/// reflexive `From<T> for T` would overlap). This type is a *response*
/// wrapper, not a propagatable error.
pub struct AutumnError {
    inner: Box<dyn std::error::Error + Send + Sync>,
    status: StatusCode,
    details: Option<std::collections::HashMap<String, Vec<String>>>,
    problem_type: Option<&'static str>,
}

/// Convenience alias -- the standard return type for Autumn handlers.
///
/// Equivalent to `Result<T, AutumnError>`. Use this as the return type
/// for any handler that might fail.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn index() -> AutumnResult<&'static str> {
///     Ok("hello")
/// }
/// ```
pub type AutumnResult<T> = Result<T, AutumnError>;

impl<E> From<E> for AutumnError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::INTERNAL_SERVER_ERROR,
            details: None,
            problem_type: None,
        }
    }
}

impl AutumnError {
    /// Override the HTTP status code.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err: AutumnError = std::io::Error::other("forbidden").into();
    /// let err = err.with_status(StatusCode::FORBIDDEN);
    /// assert_eq!(err.status(), StatusCode::FORBIDDEN);
    /// ```
    #[must_use]
    pub const fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Create a `500 Internal Server Error`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::internal_server_error(std::io::Error::other("boom"));
    /// assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    /// ```
    pub fn internal_server_error(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::INTERNAL_SERVER_ERROR,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `404 Not Found` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::not_found(std::io::Error::other("no such user"));
    /// assert_eq!(err.status(), StatusCode::NOT_FOUND);
    /// ```
    pub fn not_found(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::NOT_FOUND,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `400 Bad Request` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::bad_request(std::io::Error::other("invalid input"));
    /// assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    /// ```
    pub fn bad_request(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::BAD_REQUEST,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `422 Unprocessable Entity` error.
    ///
    /// Use this for validation failures where the request is syntactically
    /// valid but semantically incorrect.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::unprocessable(std::io::Error::other("age must be positive"));
    /// assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    /// ```
    pub fn unprocessable(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::UNPROCESSABLE_ENTITY,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `503 Service Unavailable` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::service_unavailable(std::io::Error::other("pool exhausted"));
    /// assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
    /// ```
    pub fn service_unavailable(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::SERVICE_UNAVAILABLE,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `401 Unauthorized` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::unauthorized(std::io::Error::other("not logged in"));
    /// assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    /// ```
    pub fn unauthorized(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::UNAUTHORIZED,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `403 Forbidden` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::forbidden(std::io::Error::other("not allowed"));
    /// assert_eq!(err.status(), StatusCode::FORBIDDEN);
    /// ```
    pub fn forbidden(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::FORBIDDEN,
            details: None,
            problem_type: None,
        }
    }

    /// Create a `422 Unprocessable Entity` error with field-level
    /// validation details.
    ///
    /// Use this when a request fails multiple field-specific validation rules
    /// (e.g., in a form submission). It attaches the `details` parameter, a mapping
    /// of field names to their respective error messages, so the client can display
    /// errors next to the relevant inputs.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    /// use std::collections::HashMap;
    ///
    /// let mut errors = HashMap::new();
    /// errors.insert("username".to_string(), vec!["Username is taken".to_string()]);
    ///
    /// let err = AutumnError::validation(errors);
    /// assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    /// ```
    #[must_use]
    pub fn validation(details: std::collections::HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: Box::new(StringError("Validation failed".into())),
            status: StatusCode::UNPROCESSABLE_ENTITY,
            details: Some(details),
            problem_type: None,
        }
    }

    // ── String-message convenience constructors ────────────────

    /// Create a `500 Internal Server Error` from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::internal_server_error_msg("Database explosion");
    /// assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    /// ```
    pub fn internal_server_error_msg(msg: impl Into<String>) -> Self {
        Self::internal_server_error(StringError(msg.into()))
    }

    /// Create a `404 Not Found` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::not_found_msg("No such user");
    /// assert_eq!(err.status(), StatusCode::NOT_FOUND);
    /// assert_eq!(err.to_string(), "No such user");
    /// ```
    pub fn not_found_msg(msg: impl Into<String>) -> Self {
        Self::not_found(StringError(msg.into()))
    }

    /// Create a `400 Bad Request` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::bad_request_msg("Invalid input parameter");
    /// assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    /// ```
    pub fn bad_request_msg(msg: impl Into<String>) -> Self {
        Self::bad_request(StringError(msg.into()))
    }

    /// Create a `422 Unprocessable Entity` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::unprocessable_msg("Title is required");
    /// assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    /// ```
    pub fn unprocessable_msg(msg: impl Into<String>) -> Self {
        Self::unprocessable(StringError(msg.into()))
    }

    /// Create a `401 Unauthorized` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::unauthorized_msg("Please log in to continue");
    /// assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    /// ```
    pub fn unauthorized_msg(msg: impl Into<String>) -> Self {
        Self::unauthorized(StringError(msg.into()))
    }

    /// Create a `403 Forbidden` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::forbidden_msg("You lack admin privileges");
    /// assert_eq!(err.status(), StatusCode::FORBIDDEN);
    /// ```
    pub fn forbidden_msg(msg: impl Into<String>) -> Self {
        Self::forbidden(StringError(msg.into()))
    }

    /// Create a `503 Service Unavailable` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::service_unavailable_msg("Database connection pool exhausted");
    /// assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
    /// ```
    pub fn service_unavailable_msg(msg: impl Into<String>) -> Self {
        Self::service_unavailable(StringError(msg.into()))
    }

    /// Create a `409 Conflict` error.
    ///
    /// Use this for optimistic-lock conflicts surfaced by repository `update`
    /// calls when the client's expected version is stale.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::conflict(std::io::Error::other("stale version"));
    /// assert_eq!(err.status(), StatusCode::CONFLICT);
    /// ```
    pub fn conflict(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(err),
            status: StatusCode::CONFLICT,
            details: None,
            problem_type: Some("https://autumn.dev/problems/conflict"),
        }
    }

    /// Create a `409 Conflict` error from a plain string message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err = AutumnError::conflict_msg("Concurrent edit: please reload and retry");
    /// assert_eq!(err.status(), StatusCode::CONFLICT);
    /// ```
    pub fn conflict_msg(msg: impl Into<String>) -> Self {
        Self::conflict(StringError(msg.into()))
    }

    /// Returns the HTTP status code associated with this error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::error::AutumnError;
    /// use http::StatusCode;
    ///
    /// let err: AutumnError = std::io::Error::other("boom").into();
    /// assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    /// ```
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// Return the wrapped error's source chain as displayable messages.
    ///
    /// The top-level [`AutumnError`] display already prints the wrapped error
    /// message, so this list starts at that wrapped error's first source.
    #[must_use]
    pub fn source_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        let mut source = self.inner.source();
        while let Some(error) = source {
            chain.push(error.to_string());
            source = error.source();
        }
        chain
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
            .field("details", &self.details)
            .field("problem_type", &self.problem_type)
            .finish()
    }
}

impl IntoResponse for AutumnError {
    fn into_response(self) -> Response {
        let status = self.status;
        let message = self.inner.to_string();

        // Stash error metadata for exception filters to inspect without
        // parsing the response body.
        let error_info = crate::middleware::AutumnErrorInfo {
            status,
            message: message.clone(),
            details: self.details.clone(),
        };

        if let Some(type_uri) = self.problem_type {
            let body = ProblemBody {
                type_uri,
                title: "Conflict",
                status: status.as_u16(),
                detail: message,
            };
            let mut response = (status, axum::Json(body)).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            );
            response.headers_mut().insert(
                "HX-Trigger",
                HeaderValue::from_static(r#"{"autumn:conflict":true}"#),
            );
            response.extensions_mut().insert(error_info);
            return response;
        }

        let body = ErrorBody {
            error: ErrorInner {
                status: status.as_u16(),
                message,
                details: self.details,
            },
        };

        let mut response = (status, axum::Json(body)).into_response();
        response.extensions_mut().insert(error_info);
        response
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

    #[derive(Debug)]
    struct WrappedError {
        message: String,
        source: TestError,
    }

    impl std::fmt::Display for WrappedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for WrappedError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    #[test]
    fn blanket_from_defaults_to_500() {
        let err: AutumnError = TestError("boom".into()).into();
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn internal_server_error_is_500() {
        let err = AutumnError::internal_server_error(TestError("boom".into()));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_not_found_error() {
        let err = AutumnError::not_found(std::io::Error::other("no such user"));
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
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
    fn unauthorized_is_401() {
        let err = AutumnError::unauthorized(TestError("unauthorized".into()));
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn forbidden_is_403() {
        let err = AutumnError::forbidden(TestError("forbidden".into()));
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn validation_is_422() {
        let mut details = std::collections::HashMap::new();
        details.insert("field".to_string(), vec!["error".to_string()]);
        let err = AutumnError::validation(details);
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn service_unavailable_is_503() {
        let err = AutumnError::service_unavailable(TestError("pool exhausted".into()));
        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn internal_server_error_msg_is_500() {
        let err = AutumnError::internal_server_error_msg("db failure");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.to_string(), "db failure");
    }

    #[test]
    fn not_found_msg_is_404() {
        let err = AutumnError::not_found_msg("no such user");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        assert_eq!(err.to_string(), "no such user");
    }

    #[test]
    fn bad_request_msg_is_400() {
        let err = AutumnError::bad_request_msg("invalid input");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unprocessable_msg_is_422() {
        let err = AutumnError::unprocessable_msg("title required");
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn unauthorized_msg_is_401() {
        let err = AutumnError::unauthorized_msg("login required");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn forbidden_msg_is_403() {
        let err = AutumnError::forbidden_msg("no access");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn service_unavailable_msg_is_503() {
        let err = AutumnError::service_unavailable_msg("db down");
        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.to_string(), "db down");
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
    fn source_chain_lists_inner_sources() {
        let err = AutumnError::internal_server_error(WrappedError {
            message: "failed to backfill".to_string(),
            source: TestError("database connection dropped".to_string()),
        });

        assert_eq!(
            err.source_chain(),
            vec!["database connection dropped".to_string()]
        );
    }

    #[test]
    fn into_response_has_correct_status() {
        let err = AutumnError::not_found(TestError("not found".into()));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn into_response_has_json_body() -> Result<(), axum::Error> {
        let err = AutumnError::not_found(TestError("not found".into()));
        let response = err.into_response();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");

        assert_eq!(json["error"]["status"], 404);
        assert_eq!(json["error"]["message"], "not found");
        Ok(())
    }

    #[test]
    fn debug_shows_status_and_inner() {
        let err = AutumnError::bad_request(TestError("oops".into()));
        let debug = format!("{err:?}");
        assert!(debug.contains("AutumnError"));
        assert!(debug.contains("400"));
    }

    #[tokio::test]
    async fn msg_constructor_produces_valid_json_response() -> Result<(), axum::Error> {
        let err = AutumnError::unprocessable_msg("title required");
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(json["error"]["status"], 422);
        assert_eq!(json["error"]["message"], "title required");
        Ok(())
    }

    #[tokio::test]
    async fn service_unavailable_response_is_503() -> Result<(), axum::Error> {
        let err = AutumnError::service_unavailable_msg("db down");
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(json["error"]["status"], 503);
        assert_eq!(json["error"]["message"], "db down");
        Ok(())
    }

    #[test]
    fn conflict_is_409() {
        let err = AutumnError::conflict(TestError("stale version".into()));
        assert_eq!(err.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn conflict_msg_is_409() {
        let err = AutumnError::conflict_msg("please reload and retry");
        assert_eq!(err.status(), StatusCode::CONFLICT);
        assert_eq!(err.to_string(), "please reload and retry");
    }

    #[tokio::test]
    async fn conflict_response_is_409_json() -> Result<(), axum::Error> {
        let err = AutumnError::conflict_msg("version mismatch");
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(json["status"], 409);
        assert_eq!(json["detail"], "version mismatch");
        assert_eq!(json["type"], "https://autumn.dev/problems/conflict");
        assert_eq!(json["title"], "Conflict");
        Ok(())
    }

    #[tokio::test]
    async fn conflict_response_has_hx_trigger_header() -> Result<(), axum::Error> {
        let err = AutumnError::conflict_msg("version mismatch");
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let hx_trigger = response
            .headers()
            .get("HX-Trigger")
            .expect("HX-Trigger header present");
        assert_eq!(hx_trigger, r#"{"autumn:conflict":true}"#);
        Ok(())
    }
}
