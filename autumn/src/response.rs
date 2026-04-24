pub use axum::response::{IntoResponse, Response, Html, Redirect, AppendHeaders};
pub use axum::Json;

/// Wrapper to allow returning plain primitives (like numbers) from route handlers.
pub struct Primitive<T>(pub T);

impl<T> IntoResponse for Primitive<T>
where
    T: ToString,
{
    fn into_response(self) -> Response {
        self.0.to_string().into_response()
    }
}
