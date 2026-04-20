//! WebSocket live feed — real-time notifications for post and comment activity.
//!
//! Demonstrates: `#[ws]` macro, `Channels` pub/sub, `CancellationToken`
//! for graceful shutdown, and the durable app-db relay that keeps separate
//! web and worker processes on the same live-feed stream.

use autumn_web::prelude::*;
use autumn_web::extract::State;
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

/// JSON health snapshot for the durable live-feed relay.
///
/// Operators can poll this endpoint to see which wake path the process is
/// currently using, whether reconnect attempts are happening, and whether the
/// relay has recently replayed durable events.
#[get("/api/live/relay/health")]
pub async fn live_feed_health(
    State(state): State<AppState>,
) -> AutumnResult<Json<crate::live_events::LiveFeedRelayHealthSnapshot>> {
    crate::live_events::live_feed_relay_health_snapshot(&state)
        .map(Json)
        .ok_or_else(|| {
            AutumnError::service_unavailable_msg(
                "reddit-clone live-feed relay health is not installed",
            )
        })
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
