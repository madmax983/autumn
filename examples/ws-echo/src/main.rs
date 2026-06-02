//! Minimal WebSocket echo, broadcast chat, and SSE/htmx list-update server.
//!
//! Run:
//!
//! ```bash
//! cargo run -p ws-echo
//! ```
//!
//! Then from another terminal, using `websocat`:
//!
//! ```bash
//! websocat ws://127.0.0.1:3000/echo
//! websocat ws://127.0.0.1:3000/chat
//! ```

use autumn_web::prelude::*;
use autumn_web::ws::{Message, WebSocket, WithShutdown, WsHandler};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

static NEXT_ITEM_ID: AtomicU64 = AtomicU64::new(1);

#[derive(serde::Deserialize)]
struct NewItem {
    body: String,
}

fn item_fragment(id: u64, body: &str) -> Markup {
    html! {
        li id=(format!("item-{id}")) {
            (body)
        }
    }
}

fn list_insert_fragment(id: u64, body: &str) -> Markup {
    html! {
        ul id="items" hx-swap-oob="beforeend" {
            (item_fragment(id, body))
        }
    }
}

#[get("/")]
async fn index() -> Markup {
    html! {
        (maud::DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "ws-echo live list" }
                script src="https://unpkg.com/htmx.org@2.0.4" {}
                script src="https://unpkg.com/htmx-ext-sse@2.2.3/sse.js" {}
            }
            body {
                main hx-ext="sse" sse-connect="/events" sse-swap="message" {
                    h1 { "Live list" }
                    form hx-post="/items" hx-swap="none" {
                        input type="text" name="body" placeholder="New item" required;
                        button type="submit" { "Post" }
                    }
                    ul id="items" {}
                }
            }
        }
    }
}

/// Bounce every text message back to the sender.
#[ws("/echo")]
async fn echo() -> impl WsHandler {
    |mut socket: WebSocket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            if let Message::Text(text) = msg
                && socket.send(Message::Text(text)).await.is_err()
            {
                break;
            }
        }
    }
}

/// Broadcast every message to everyone subscribed to the "lobby" channel.
///
/// Demonstrates `Channels` pub/sub via `AppState` and graceful shutdown
/// via the `CancellationToken` parameter.
#[ws("/chat")]
async fn chat(state: AppState) -> impl WsHandler {
    let channels = state.channels().clone();
    let tx = channels.sender("lobby");
    let mut rx = channels.subscribe("lobby");

    WithShutdown(
        |mut socket: WebSocket, shutdown: CancellationToken| async move {
            loop {
                tokio::select! {
                    incoming = socket.recv() => {
                        match incoming {
                            Some(Ok(Message::Text(text))) => {
                                tx.send(text.to_string()).ok();
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                    broadcast = rx.recv() => {
                        if let Ok(msg) = broadcast
                            && socket.send(Message::Text(msg.into_string().into())).await.is_err()
                        {
                            break;
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

/// Subscribe to htmx-ready HTML fragments over Server-Sent Events.
#[get("/events")]
async fn events(State(state): State<AppState>) -> impl IntoResponse {
    autumn_web::sse::stream(&state, "lobby-html")
}

/// Publish a Maud-rendered list item wrapped in an `hx-swap-oob` envelope.
#[post("/items")]
async fn create_item(
    State(state): State<AppState>,
    Form(item): Form<NewItem>,
) -> AutumnResult<&'static str> {
    let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
    let body = item.body.trim();
    if body.is_empty() {
        return Err(AutumnError::bad_request_msg("item body is required"));
    }

    state
        .broadcast()
        .publish_html("lobby-html", &list_insert_fragment(id, body))?;
    Ok("sent")
}

/// Backward-compatible smoke endpoint used by the Redis compose test.
#[post("/notify")]
async fn notify(State(state): State<AppState>) -> AutumnResult<&'static str> {
    let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
    state.broadcast().publish_html(
        "lobby-html",
        &list_insert_fragment(id, "Broadcast from /notify"),
    )?;
    Ok("sent")
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, echo, chat, events, create_item, notify])
        .run()
        .await;
}

#[cfg(test)]
mod mutant_tests {
    use super::*;

    #[tokio::test]
    async fn test_index_html_contains_required_elements() {
        let html = index().await.into_string();
        assert!(html.contains("Live list"));
        assert!(html.contains("hx-ext=\"sse\""));
        assert!(html.contains("sse-connect="));
    }

    #[test]
    fn test_item_fragment_structure() {
        let fragment = item_fragment(1, "test message");
        let html = fragment.into_string();
        assert!(html.contains("<li id=\"item-1\">test message</li>"));
    }

    #[test]
    fn test_list_insert_fragment_structure() {
        let fragment = list_insert_fragment(2, "inserted message");
        let html = fragment.into_string();
        assert!(html.contains("hx-swap-oob=\"beforeend\""));
        assert!(html.contains("<li id=\"item-2\">inserted message</li>"));
    }
}
