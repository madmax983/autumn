//! "Who is viewing this post?" — an end-to-end presence example.
//!
//! Demonstrates `autumn_web::presence::Presence` for live viewer badges using
//! only Maud + htmx + SSE. No hand-rolled JS state management required.
//!
//! ## Quick start
//!
//! ```bash
//! cargo run -p presence-viewers
//! ```
//!
//! Then open `http://127.0.0.1:3000/posts/1` in multiple browser tabs. The
//! viewer count updates in real time as tabs open and close.
//!
//! ## How it works
//!
//! 1. `GET /posts/{id}` renders the page, opening an SSE connection to `/posts/{id}/track`.
//! 2. The SSE endpoint calls `presence.track(topic, key, meta)` and subscribes
//!    to the derived channel `presence:post:{id}` for join/leave events.
//! 3. On each presence event it re-renders and pushes the updated viewer badge.
//! 4. When the browser closes the SSE connection the `PresenceHandle` drops,
//!    automatically broadcasting a leave event.
//! 5. `GET /api/posts/{id}/viewers` returns the JSON viewer list — works for
//!    any HTTP client, not just htmx.

use autumn_web::prelude::*;
use autumn_web::presence::{Presence, PresenceEvent};

// ── Page layout ──────────────────────────────────────────────────────────────

fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                script src="https://unpkg.com/htmx.org@2.0.4" {}
                script src="https://unpkg.com/htmx-ext-sse@2.2.3/sse.js" {}
                style {
                    "body{font-family:sans-serif;max-width:640px;margin:2rem auto;padding:0 1rem}"
                    ".badge{display:inline-flex;align-items:center;gap:.5rem;padding:.25rem .75rem;"
                    "  background:#f0f4ff;border-radius:999px;font-size:.875rem;color:#555}"
                    ".dot{width:8px;height:8px;border-radius:50%;background:#22c55e;"
                    "  animation:pulse 1.5s infinite}"
                    "@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}"
                }
            }
            body { (content) }
        }
    }
}

// ── Viewer badge fragment ─────────────────────────────────────────────────────

fn viewer_badge(count: usize) -> Markup {
    html! {
        span id="viewer-badge" class="badge" {
            span class="dot" {}
            @if count == 1 { "1 viewer" } @else { (count) " viewers" }
        }
    }
}

// ── Routes ───────────────────────────────────────────────────────────────────

/// Render a blog post with a live viewer badge driven by SSE.
#[get("/posts/{id}")]
async fn show_post(path: Path<u32>) -> Markup {
    let post_id = *path;
    let track_url = format!("/posts/{post_id}/track");

    layout(
        &format!("Post #{post_id}"),
        html! {
            h1 { "Post #" (post_id) }
            p {
                "Open this page in multiple browser tabs to see the live viewer count update."
            }
            // hx-ext="sse" opens the SSE connection; sse-swap="viewer-badge"
            // swaps the element whose id matches the SSE event name.
            div hx-ext="sse"
                sse-connect=(track_url)
                sse-swap="viewer-badge"
                hx-target="#viewer-badge"
                hx-swap="outerHTML" {
                (viewer_badge(0))
            }
        },
    )
}

/// SSE endpoint: register this tab as a viewer and stream badge updates.
///
/// The `Presence` extractor is injected by the framework from the shared
/// process-level store. Calling `presence.track(...)` registers this
/// connection; the returned `PresenceHandle` lives until the SSE stream
/// drops (i.e., the browser closes the connection), at which point it
/// automatically broadcasts a leave event.
#[get("/posts/{id}/track")]
async fn track_viewer(
    State(state): State<AppState>,
    presence: Presence,
    path: Path<u32>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let post_id = *path;
    let topic = format!("post:{post_id}");
    let presence_channel = format!("presence:{topic}");

    // Subscribe before tracking so the initial join event is never missed.
    let mut rx = state.channels().subscribe(&presence_channel);

    // Register this tab. The key is unique per tab; in a real app, use the
    // authenticated user ID so multiple tabs from the same user collapse into
    // one PresenceEntry with multiple metas.
    let tab_key = format!("tab-{:08x}", timestamp_nanos());
    let meta = serde_json::json!({ "post_id": post_id });
    let handle = presence.track(&topic, &tab_key, meta);

    // Initial badge — rendered immediately so the UI shows the current count
    // without waiting for the first event.
    let initial_count = presence.list(&topic).len();
    let initial_event = Event::default()
        .event("viewer-badge")
        .data(viewer_badge(initial_count).into_string());

    let stream = async_stream::stream! {
        yield Ok(initial_event);

        loop {
            match rx.recv().await {
                Ok(msg) => {
                    // Only re-render on join/leave events (ignore unrecognised payloads).
                    if serde_json::from_str::<PresenceEvent>(msg.as_str()).is_err() {
                        continue;
                    }
                    let count = presence.list(&topic).len();
                    yield Ok(Event::default()
                        .event("viewer-badge")
                        .data(viewer_badge(count).into_string()));
                }
                Err(_) => break,
            }
        }

        // Explicit drop to clarify intent: `handle` owns this connection's
        // presence entry. When the stream ends (browser disconnects), `handle`
        // drops here, broadcasting a leave event for `tab_key`.
        drop(handle);
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// JSON API: list current viewers for a post.
///
/// Works with any HTTP client — no htmx dependency in the presence API.
///
/// ```bash
/// curl http://127.0.0.1:3000/api/posts/1/viewers
/// ```
#[get("/api/posts/{id}/viewers")]
async fn list_viewers(presence: Presence, path: Path<u32>) -> impl IntoResponse {
    let post_id = *path;
    let entries = presence.list(&format!("post:{post_id}"));
    Json(entries)
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![show_post, track_viewer, list_viewers])
        .run()
        .await;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn timestamp_nanos() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
}
