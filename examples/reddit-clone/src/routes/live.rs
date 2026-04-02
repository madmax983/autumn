//! WebSocket live feed — real-time notifications for post and comment activity.
//!
//! Demonstrates: `#[ws]` macro, `Channels` pub/sub, `CancellationToken`
//! for graceful shutdown, and the two-function pattern (pre-upgrade
//! extractor access + post-upgrade socket handling).

use autumn_web::prelude::*;
use autumn_web::ws::{Message, WebSocket, WithShutdown, WsHandler};
use tokio_util::sync::CancellationToken;

/// WebSocket endpoint for the global activity feed.
///
/// Clients connect to `/ws/feed` and receive JSON messages whenever
/// a new post or comment is created anywhere on the site. The
/// pre-upgrade function captures `AppState` for channel access;
/// the returned closure owns the live socket.
#[ws("/ws/feed")]
pub async fn live_feed(state: AppState) -> impl WsHandler {
    let channels = state.channels().clone();
    let mut rx = channels.subscribe("feed");

    WithShutdown(
        |mut socket: WebSocket, shutdown: CancellationToken| async move {
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Ok(cm) => {
                                if socket.send(Message::Text(cm.into_string().into())).await.is_err() {
                                    break; // Client disconnected
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("feed subscriber lagged by {n} messages");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    () = shutdown.cancelled() => {
                        socket.send(Message::Close(None)).await.ok();
                        break;
                    }
                }
            }
        },
    )
}

/// WebSocket endpoint for a specific subreddit's activity.
///
/// Clients connect to `/ws/r/{slug}` and receive JSON notifications
/// for posts and comments in that community only.
#[ws("/ws/r/{slug}")]
pub async fn subreddit_feed(
    slug: autumn_web::extract::Path<String>,
    state: AppState,
) -> impl WsHandler {
    let channel_name = format!("r/{}", *slug);
    let channels = state.channels().clone();
    let mut rx = channels.subscribe(&channel_name);

    WithShutdown(
        |mut socket: WebSocket, shutdown: CancellationToken| async move {
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Ok(cm) => {
                                if socket.send(Message::Text(cm.into_string().into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    () = shutdown.cancelled() => {
                        socket.send(Message::Close(None)).await.ok();
                        break;
                    }
                }
            }
        },
    )
}

/// Publish a feed event to the global and subreddit-specific channels.
///
/// Called from post/comment creation routes to broadcast activity.
#[allow(dead_code)] // Available for routes that want to broadcast events
pub fn publish_activity(state: &AppState, subreddit_slug: &str, event: &str) {
    let channels = state.channels();
    // Broadcast to global feed
    channels.sender("feed").send(event).ok();
    // Broadcast to subreddit-specific feed
    channels
        .sender(&format!("r/{subreddit_slug}"))
        .send(event)
        .ok();
}
