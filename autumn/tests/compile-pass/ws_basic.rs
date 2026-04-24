//! Compile-pass tests for the `#[ws]` macro.
//!
//! Exercises the supported handler shapes:
//! - minimal handler returning a `|WebSocket|` closure
//! - handler using `AppState` (special-cased by the macro)
//! - handler using standard Autumn extractors (`Path`, `Query`)
//! - handler using `WithShutdown` for the cancellation-token closure form

use autumn_web::extract::{Path, Query};
use autumn_web::prelude::*;
use autumn_web::routes;
use autumn_web::ws::{Message, WebSocket, WithShutdown, WsHandler};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

#[ws("/echo")]
async fn echo() -> impl WsHandler {
    |mut socket: WebSocket| async move {
        while let Some(Ok(Message::Text(t))) = socket.recv().await {
            socket.send(Message::Text(t)).await.ok();
        }
    }
}

#[ws("/chat/{room}")]
async fn chat(room: Path<String>, state: AppState) -> impl WsHandler {
    let channels = state.channels().clone();
    let name = room.to_string();
    let mut rx = channels.subscribe(&name);
    WithShutdown(
        move |mut socket: WebSocket, shutdown: CancellationToken| async move {
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        if let Ok(m) = msg {
                            socket.send(Message::Text(m.into_string().into())).await.ok();
                        }
                    }
                    () = shutdown.cancelled() => break,
                }
            }
        },
    )
}

#[derive(Deserialize)]
struct Opts {
    _token: Option<String>,
}

#[ws("/feed")]
async fn feed(_q: Query<Opts>) -> impl WsHandler {
    |mut socket: WebSocket| async move {
        socket.send(Message::Text("hi".into())).await.ok();
    }
}

fn main() {
    // routes![] must accept all three #[ws] handlers unchanged.
    let _routes = routes![echo, chat, feed];
}
