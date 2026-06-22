//! Logging infrastructure and structured telemetry formatting.
//!
//! Exposes configurable layers for `tracing` that integrate deeply into
//! the framework, handling contextual key-value extraction, HTTP request
//! capturing, and PII parameter filtering.

pub mod capture;
pub mod context;
pub mod filter;
