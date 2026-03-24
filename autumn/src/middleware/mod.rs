//! Built-in middleware for Autumn applications.
//!
//! Currently provides:
//!
//! - [`RequestIdLayer`] / [`RequestId`] -- assigns a unique UUID v4 to every
//!   incoming HTTP request, available in handler extensions and the
//!   `X-Request-Id` response header.
//!
//! The [`RequestIdLayer`] is applied automatically by
//! [`AppBuilder::run`](crate::app::AppBuilder::run); you do not need to
//! add it manually.

pub mod request_id;

pub use request_id::{RequestId, RequestIdLayer};
