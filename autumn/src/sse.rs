//! Server-Sent Events (SSE) support for Autumn applications.
//!
//! This module provides ergonomic SSE handling, integrating with Autumn's
//! ecosystem to easily yield real-time updates to the client. SSE is a lightweight
//! alternative to `WebSockets` for one-way server-to-client event streams.
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
#[cfg(feature = "ws")]
use std::convert::Infallible;
#[cfg(feature = "ws")]
use std::future::Future;
use std::time::Duration;

/// Returns a default `KeepAlive` configuration for Server-Sent Events.
///
/// Sends a keep-alive message every 15 seconds to prevent proxies or load
/// balancers from dropping the connection during idle periods.
pub fn keep_alive() -> KeepAlive {
    KeepAlive::new().interval(Duration::from_secs(15))
}

/// Convert a channel subscriber into an SSE response stream.
#[cfg(feature = "ws")]
pub fn from_subscriber(
    subscriber: crate::channels::Subscriber,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + use<>> {
    use tokio_stream::StreamExt;

    let stream = subscriber
        .into_stream()
        .map(|msg| Ok(Event::default().data(msg.into_string())));
    Sse::new(stream).keep_alive(keep_alive())
}

/// Subscribe to a channel topic and return an SSE response stream.
///
/// This is the one-line route primitive for htmx's SSE extension:
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/events")]
/// async fn events(State(state): State<AppState>) -> impl IntoResponse {
///     autumn_web::sse::stream(&state, "feed")
/// }
/// ```
#[cfg(feature = "ws")]
pub fn stream(
    state: &crate::AppState,
    topic: &str,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + use<>> {
    from_subscriber(state.channels().subscribe(topic))
}

/// Authorize an SSE channel subscription before the subscriber is created.
///
/// This preserves the "outer handler checks access, returned stream owns the
/// live client" shape used by Autumn's WebSocket support.
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/events")]
/// async fn events(
///     State(state): State<AppState>,
///     session: Session,
/// ) -> AutumnResult<impl IntoResponse> {
///     autumn_web::sse::stream_authorized(&state, "private-feed", |_| async move {
///         if session.contains_key("user_id").await {
///             Ok(())
///         } else {
///             Err(AutumnError::unauthorized_msg("login required"))
///         }
///     })
///     .await
/// }
/// ```
///
/// # Errors
///
/// Returns the error produced by the authorization hook.
#[cfg(feature = "ws")]
pub async fn stream_authorized<E, F, Fut>(
    state: &crate::AppState,
    topic: &str,
    authorize: F,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + use<E, F, Fut>>, E>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let subscriber = state
        .channels()
        .subscribe_authorized(topic, authorize)
        .await?;
    Ok(from_subscriber(subscriber))
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

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn stream_helper_builds_sse_from_app_state_channels() {
        let state = crate::AppState::for_test();
        let _sse = stream(&state, "lobby");
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn stream_authorized_rejects_before_subscription() {
        let state = crate::AppState::for_test();

        let result = stream_authorized(&state, "private", |topic| async move {
            assert_eq!(topic, "private");
            Err::<(), &'static str>("denied")
        })
        .await;

        assert!(matches!(result, Err("denied")));
        assert!(!state.channels().snapshot().contains_key("private"));
    }
}
