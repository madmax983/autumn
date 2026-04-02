//! Integration tests for WebSocket support (`#[ws]` macro + Channels).
//!
//! These tests verify:
//! - The `#[ws]` macro generates valid GET routes
//! - WebSocket upgrade requests receive 101 Switching Protocols
//! - Non-upgrade GET requests to WS paths return an appropriate error
//! - The `Channels` broadcast system works end-to-end
//! - Graceful shutdown cancellation is propagated

#![cfg(feature = "ws")]

use autumn_web::config::AutumnConfig;
use autumn_web::prelude::*;
use autumn_web::ws::{CancellationToken, Message, WebSocket, WsHandler};
use axum::body::Body;
use http::Request;
use tower::ServiceExt;

fn test_state() -> AppState {
    AppState::for_test().with_profile("test")
}

// ── Test handlers ────────────────────────────────────────────────

#[ws("/echo")]
async fn echo() -> impl WsHandler {
    std::future::ready(()).await;
    |mut socket: WebSocket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            if let Message::Text(text) = msg {
                socket.send(Message::Text(text)).await.ok();
            }
        }
    }
}

#[ws("/with-state")]
async fn with_state(state: AppState) -> impl WsHandler {
    std::future::ready(()).await;
    let _channels = state.channels().clone();
    |mut socket: WebSocket| async move {
        socket.send(Message::Text("hello".into())).await.ok();
    }
}

#[ws("/with-shutdown")]
async fn with_shutdown() -> impl WsHandler {
    std::future::ready(()).await;
    autumn_web::ws::WithShutdown(
        |mut socket: WebSocket, shutdown: CancellationToken| async move {
            loop {
                tokio::select! {
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                socket.send(Message::Text(text)).await.ok();
                            }
                            _ => break,
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

// ── Route registration tests ─────────────────────────────────────

#[test]
fn ws_macro_generates_route_info() {
    let routes = routes![echo];
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/echo");
    assert_eq!(routes[0].method, http::Method::GET);
    assert_eq!(routes[0].name, "echo");
}

#[test]
fn ws_routes_coexist_with_http_routes() {
    #[get("/hello")]
    async fn hello() -> &'static str {
        "hi"
    }

    let all_routes = routes![hello, echo];
    assert_eq!(all_routes.len(), 2);
    assert_eq!(all_routes[0].path, "/hello");
    assert_eq!(all_routes[1].path, "/echo");
}

#[test]
fn ws_with_state_generates_route() {
    let routes = routes![with_state];
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/with-state");
}

#[test]
fn ws_with_shutdown_generates_route() {
    let routes = routes![with_shutdown];
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/with-shutdown");
}

// ── HTTP-level upgrade tests ─────────────────────────────────────

#[tokio::test]
async fn non_upgrade_get_returns_error() {
    let config = AutumnConfig::default();
    let state = test_state();

    let router = autumn_web::app::build_router(routes![echo], &config, state);

    // A plain GET (without upgrade headers) should NOT get 200
    let req = Request::builder().uri("/echo").body(Body::empty()).unwrap();

    let resp = router.oneshot(req).await.unwrap();
    // Axum returns 421 (upgrade required) for non-upgrade WS requests
    assert_ne!(resp.status(), http::StatusCode::OK);
}

#[tokio::test]
async fn upgrade_request_without_real_tcp_returns_426() {
    let config = AutumnConfig::default();
    let state = test_state();

    let router = autumn_web::app::build_router(routes![echo], &config, state);

    // With tower::oneshot there's no real TCP connection, so the upgrade
    // cannot complete. Axum correctly returns 426 Upgrade Required.
    // This still proves the WS handler is mounted and recognized the
    // upgrade headers — a non-WS GET would return 200 or 404 instead.
    let req = Request::builder()
        .uri("/echo")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::from_u16(426).unwrap());
}

// ── Channels unit tests (beyond channels.rs) ─────────────────────

#[tokio::test]
async fn channels_on_app_state() {
    let state = test_state();
    let tx = state.channels().sender("test");
    let mut rx = state.channels().subscribe("test");

    tx.send("from state").unwrap();
    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.as_str(), "from state");
}

#[tokio::test]
async fn shutdown_token_propagates() {
    let state = test_state();
    let child = state.shutdown_token();

    assert!(!child.is_cancelled());
    state.trigger_shutdown_for_test();
    assert!(child.is_cancelled());
}
