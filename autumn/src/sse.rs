//! Server-Sent Events (SSE) support for Autumn applications.
//!
//! This module provides ergonomic SSE handling, integrating with Autumn's
//! ecosystem to easily yield real-time updates to the client. SSE is a lightweight
//! alternative to WebSockets for one-way server-to-client event streams.
//!
//! # Examples
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::sse::{Sse, Event, keep_alive};
//! use futures::stream::Stream;
//! use std::convert::Infallible;
//!
//! #[get("/stream")]
//! async fn stream(state: AppState) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
//!     let mut rx = state.channels().subscribe("lobby");
//!
//!     let stream = async_stream::stream! {
//!         while let Ok(msg) = rx.recv().await {
//!             yield Ok(Event::default().data(msg.into_string()));
//!         }
//!     };
//!
//!     Sse::new(stream).keep_alive(keep_alive())
//! }
//! ```

pub use axum::response::sse::{Event, KeepAlive, Sse};
use std::time::Duration;

/// Returns a default `KeepAlive` configuration for Server-Sent Events.
///
/// Sends a keep-alive message every 15 seconds to prevent proxies or load
/// balancers from dropping the connection during idle periods.
pub fn keep_alive() -> KeepAlive {
    KeepAlive::new().interval(Duration::from_secs(15))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keep_alive_default() {
        let ka = keep_alive();
        // Since KeepAlive fields are private in axum, we just ensure it constructs successfully.
        // We can format it to string using debug, to verify it's properly initialized.
        let debug_str = format!("{ka:?}");
        assert!(debug_str.contains("KeepAlive"));
    }
}
