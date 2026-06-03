//! Extractor and responder for CSV payloads.
//!
//! Provides the [`Csv`] wrapper type that allows parsing request bodies
//! as CSV and serializing responses as CSV, similar to how [`axum::Json`] works.
//!
//! Requires the `csv` feature flag.

use crate::AutumnError;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

/// Deserialize and serialize CSV request/response bodies.
///
/// Wraps [`csv::Reader`] and [`csv::Writer`] so CSV parse failures use Autumn's
/// Problem Details error contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct Csv<T>(pub T);

impl<T> std::ops::Deref for Csv<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Csv<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S, T> FromRequest<S> for Csv<Vec<T>>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned + Send + Sync,
{
    type Rejection = AutumnError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|err| AutumnError::bad_request_msg(err.to_string()))?;

        let mut rdr = csv::Reader::from_reader(bytes.as_ref());
        let mut records = Vec::new();

        for result in rdr.deserialize() {
            match result {
                Ok(record) => records.push(record),
                Err(err) => {
                    return Err(AutumnError::bad_request_msg(format!(
                        "CSV parse error: {err}"
                    )));
                }
            }
        }

        Ok(Self(records))
    }
}

impl<T> IntoResponse for Csv<Vec<T>>
where
    T: serde::Serialize,
{
    fn into_response(self) -> Response {
        let mut wtr = csv::Writer::from_writer(vec![]);
        for record in &self.0 {
            if let Err(err) = wtr.serialize(record) {
                return AutumnError::internal_server_error_msg(format!(
                    "Failed to serialize CSV: {err}"
                ))
                .into_response();
            }
        }

        match wtr.into_inner() {
            Ok(bytes) => (
                [(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("text/csv; charset=utf-8"),
                )],
                bytes,
            )
                .into_response(),
            Err(err) => {
                AutumnError::internal_server_error_msg(format!("Failed to flush CSV: {err}"))
                    .into_response()
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::header::CONTENT_TYPE;
    use axum::http::StatusCode;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
    struct Record {
        name: String,
        age: u32,
    }

    #[tokio::test]
    async fn extractor_parses_csv_body() {
        let body = "name,age\nAlice,30\nBob,25\n";
        let req = Request::builder()
            .header(CONTENT_TYPE, "text/csv")
            .body(Body::from(body))
            .unwrap();

        let Csv(records): Csv<Vec<Record>> = Csv::from_request(req, &())
            .await
            .expect("should parse valid CSV");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "Alice");
        assert_eq!(records[0].age, 30);
    }

    #[tokio::test]
    async fn extractor_returns_autumn_error_on_invalid_csv() {
        let body = "name,age\nAlice,not-a-number\n";
        let req = Request::builder()
            .header(CONTENT_TYPE, "text/csv")
            .body(Body::from(body))
            .unwrap();

        let err = Csv::<Vec<Record>>::from_request(req, &())
            .await
            .unwrap_err();

        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        assert!(
            err.to_string().contains("CSV deserialization error")
                || err.to_string().contains("CSV parse error")
        );
    }

    #[tokio::test]
    async fn responder_serializes_to_csv() {
        let records = vec![
            Record {
                name: "Alice".into(),
                age: 30,
            },
            Record {
                name: "Bob".into(),
                age: 25,
            },
        ];

        let response = Csv(records).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

        assert_eq!(body_str, "name,age\nAlice,30\nBob,25\n");
    }
}
