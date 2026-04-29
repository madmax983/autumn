//! Minimal WebSocket echo + broadcast chat server.
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
use tokio_util::sync::CancellationToken;

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

#[autumn_web::main]
async fn main() {
    autumn_web::app().routes(routes![echo, chat]).run().await;
}
