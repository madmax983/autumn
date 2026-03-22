//! Re-exports of Axum extractors for use in Autumn handlers.
//!
//! These are provided so users don't need `axum` as a direct dependency.
//!
//! `Json<T>` serves double duty — it's both an extractor (parses JSON request
//! bodies) and a response type (serializes to JSON with `application/json`
//! content type).

pub use axum::extract::{Form, Json, Path, Query};
