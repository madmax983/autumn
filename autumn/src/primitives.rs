use axum::response::{IntoResponse, Response};
use http::StatusCode;

/// A newtype wrapper for returning plain primitive types from route handlers.
///
/// Out of the box, returning an `i32` or `f64` from an `async fn` handler
/// will fail to compile with trait bound errors because Axum does not implement
/// `IntoResponse` for numeric primitives. Wrapping the value in `Primitive`
/// ensures it responds with `200 OK` and a stringified `text/plain` body.
pub struct Primitive<T>(pub T);

macro_rules! impl_into_response_for_primitive {
    ($($ty:ty),*) => {
        $(
            impl IntoResponse for Primitive<$ty> {
                fn into_response(self) -> Response {
                    let body = self.0.to_string();
                    let mut response = (StatusCode::OK, body).into_response();
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
                    );
                    response
                }
            }
        )*
    };
}

impl_into_response_for_primitive!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64, bool
);

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn primitive_i32_into_response() {
        let resp = Primitive(42i32).into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"42");
    }
}
