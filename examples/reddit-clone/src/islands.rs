//! WASM Islands — interactive client-side components.
//!
//! Demonstrates: `#[island]` macro for client-hydrated components,
//! `#[server]` macro for server actions callable from WASM,
//! `autumn_web::wasm::island()` for rendering islands in Maud templates.
//!
//! The vote counter island renders a fully interactive upvote/downvote
//! widget that hydrates on the client via WebAssembly while still
//! working as a plain HTML form with htmx as a fallback.
//!
//! # Architecture
//!
//! Islands follow a two-tier rendering strategy:
//!
//! 1. **Server**: Renders fallback HTML (with htmx for no-WASM browsers)
//! 2. **Browser**: Hydrates the island with reactive WASM code
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_wasm::prelude::*;
//!
//! #[derive(Clone, serde::Serialize, serde::Deserialize)]
//! pub struct VoteCounterProps {
//!     pub post_id: i64,
//!     pub score: i64,
//! }
//!
//! // Renders on the client as a reactive WASM island
//! #[island]
//! fn vote_counter(cx: IslandCx<VoteCounterProps>) {
//!     let score = Signal::new(cx.props.score);
//!
//!     // Reactive: re-renders automatically when score changes
//!     html! {
//!         div class="vote-controls" {
//!             button on:click={
//!                 let score = score.clone();
//!                 move |_| {
//!                     wasm_bindgen_futures::spawn_local(async move {
//!                         if let Ok(result) = cast_vote(CastVoteInput {
//!                             post_id: cx.props.post_id,
//!                             direction: 1,
//!                         }).await {
//!                             score.set(result.score);
//!                         }
//!                     });
//!                 }
//!             } { "▲" }
//!             span { (score.get()) }
//!             button on:click={/* downvote */} { "▼" }
//!         }
//!     }
//! }
//!
//! // Server action: runs on server, callable from WASM via fetch
//! #[server]
//! async fn cast_vote(input: CastVoteInput) -> AutumnResult<CastVoteOutput> {
//!     // Database mutation runs server-side
//!     // WASM client calls this via /_autumn/actions/cast_vote
//!     Ok(CastVoteOutput { score: 42 })
//! }
//! ```
//!
//! # Registration
//!
//! ```rust,ignore
//! // In main.rs:
//! autumn_web::app()
//!     .islands(islands![vote_counter])
//!     .actions(actions![cast_vote])
//!     .run()
//!     .await;
//! ```
//!
//! # Browser bootstrap (in WASM entry point)
//!
//! ```rust,ignore
//! use autumn_wasm::prelude::*;
//!
//! #[wasm_bindgen(start)]
//! pub fn main() {
//!     boot(&[vote_counter::registration()]);
//! }
//! ```

use autumn_web::prelude::*;

/// Props for the vote counter island component.
#[allow(dead_code)] // Used by WASM build target
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VoteCounterProps {
    pub post_id: i64,
    pub score: i64,
}

/// Render a vote counter as an island-ready element with htmx fallback.
///
/// In browsers with WASM support, the `data-autumn-island` attribute
/// triggers hydration. Without WASM, the htmx attributes provide the
/// same voting functionality via server round-trips.
#[allow(dead_code)] // Available for templates that opt into island rendering
pub fn vote_island(post_id: i64, score: i64) -> Markup {
    let props = VoteCounterProps { post_id, score };
    let fallback = crate::routes::layout::vote_controls(post_id, score);

    autumn_web::wasm::island(
        autumn_web::wasm::IslandMeta {
            name: "vote-counter",
            mount_id: "vote-mount",
            props_type: "VoteCounterProps",
        },
        props,
        fallback,
    )
}
