//! WebSocket support for Autumn applications.
//!
//! This module provides ergonomic WebSocket handling through the [`#[ws]`](macro@crate::ws)
//! macro and re-exports of Axum's WebSocket types.
//!
//! # Two-function pattern
//!
//! WebSocket handlers in Autumn use a **two-function pattern**: the outer
//! function runs at HTTP upgrade time (before the WebSocket connection is
//! established) and returns a closure that handles the live socket.
//!
//! This split gives you:
//! - **Pre-upgrade access** to Axum extractors (auth, session, state)
//! - **Post-upgrade ownership** of the `WebSocket` + captured values
//! - A natural place for connection rejection (return an error before upgrade)
//!
//! # Examples
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::ws::{WebSocket, Message};
//!
//! // Simple echo server
//! #[ws("/echo")]
//! async fn echo() -> impl WsHandler {
//!     |mut socket: WebSocket| async move {
//!         while let Some(Ok(msg)) = socket.recv().await {
//!             if let Message::Text(text) = msg {
//!                 socket.send(Message::Text(text)).await.ok();
//!             }
//!         }
//!     }
//! }
//!
//! // With state and graceful shutdown
//! #[ws("/chat")]
//! async fn chat(state: AppState) -> impl WsHandler {
//!     let channels = state.channels();
//!     let tx = channels.sender("lobby");
//!     let mut rx = channels.subscribe("lobby");
//!
//!     |mut socket: WebSocket, shutdown: CancellationToken| async move {
//!         loop {
//!             tokio::select! {
//!                 Some(Ok(Message::Text(text))) = socket.recv() => {
//!                     tx.send(text.to_string()).ok();
//!                 }
//!                 Ok(msg) = rx.recv() => {
//!                     socket.send(Message::Text(msg.into())).await.ok();
//!                 }
//!                 _ = shutdown.cancelled() => {
//!                     socket.send(Message::Close(None)).await.ok();
//!                     break;
//!                 }
//!             }
//!         }
//!     }
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

pub use axum::extract::WebSocketUpgrade;
pub use axum::extract::ws::{CloseCode, CloseFrame, Message, Utf8Bytes, WebSocket};
pub use tokio_util::sync::CancellationToken;

/// Trait for WebSocket connection handlers.
///
/// Implemented automatically for closures matching the supported signatures.
/// Users never implement this trait directly — they return closures from
/// `#[ws]` handler functions.
///
/// # Supported signatures
///
/// ```rust,ignore
/// // Minimal: just the socket
/// |socket: WebSocket| async move { /* ... */ }
///
/// // With shutdown signal
/// |socket: WebSocket, shutdown: CancellationToken| async move { /* ... */ }
/// ```
pub trait WsHandler: Send + 'static {
    /// Handle an upgraded WebSocket connection.
    fn handle(
        self,
        socket: WebSocket,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

// ── Blanket impl: closure taking (WebSocket) ───────────────────────

impl<F, Fut> WsHandler for F
where
    F: FnOnce(WebSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn handle(
        self,
        socket: WebSocket,
        _shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin((self)(socket))
    }
}

// NOTE: We cannot have a second blanket impl for `FnOnce(WebSocket, CancellationToken)`
// because it conflicts with the above. Instead, we provide a newtype wrapper.

/// Wrapper that enables `|socket, shutdown|` closures as [`WsHandler`].
///
/// Users don't construct this directly. The `#[ws]` macro detects the
/// `CancellationToken` parameter in the closure and wraps it automatically.
/// For manual usage:
///
/// ```rust,ignore
/// use autumn_web::ws::{WithShutdown, WebSocket, CancellationToken};
///
/// let handler = WithShutdown(|socket: WebSocket, shutdown: CancellationToken| async move {
///     // ...
/// });
/// ```
pub struct WithShutdown<F>(pub F);

impl<F, Fut> WsHandler for WithShutdown<F>
where
    F: FnOnce(WebSocket, CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn handle(
        self,
        socket: WebSocket,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin((self.0)(socket, shutdown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_handler_is_object_safe_enough() {
        // Verify the trait can be used as a generic bound
        fn accept_handler<H: WsHandler>(_h: H) {}

        let handler = |_socket: WebSocket| async {};
        accept_handler(handler);
    }

    #[test]
    fn with_shutdown_compiles() {
        fn accept_handler<H: WsHandler>(_h: H) {}

        let handler = WithShutdown(|_socket: WebSocket, _shutdown: CancellationToken| async {});
        accept_handler(handler);
    }
}
