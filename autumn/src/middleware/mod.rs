//! Built-in middleware for Autumn applications.
//!
//! Currently provides:
//!
//! - [`RequestIdLayer`] / [`RequestId`] -- assigns a unique UUID v4 to every
//!   incoming HTTP request, available in handler extensions and the
//!   `X-Request-Id` response header.
//! - [`ExceptionFilterLayer`] / [`ExceptionFilter`] -- intercepts error
//!   responses and runs a user-registered filter chain for logging,
//!   transformation, or replacement.
//!
//! The [`RequestIdLayer`] is applied automatically by
//! [`AppBuilder::run`](crate::app::AppBuilder::run); you do not need to
//! add it manually. The [`ExceptionFilterLayer`] is applied automatically
//! when at least one exception filter is registered via
//! [`AppBuilder::exception_filter`](crate::app::AppBuilder::exception_filter).

pub mod exception_filter;
pub mod request_id;

pub use exception_filter::{AutumnErrorInfo, ExceptionFilter, ExceptionFilterLayer};
pub use request_id::{RequestId, RequestIdLayer};
