// ── v0.2 Feature: #[repository] macro ───────────────────────────
//
// Generates PgSubredditRepository and PgPostRepository with:
//   - Auto-generated CRUD methods (find_by_id, find_all, save, etc.)
//   - Derived queries parsed from method signatures
//   - FromRequestParts extractor (use as handler parameter)
//   - Optional REST API handler generation with `api = "..."`
//   - Optional mutation hooks with `hooks = ...`
//   - `broadcasts = "posts"` on PostRepository publishes hx-swap-oob fragments
//     over the "posts" SSE topic when mutations go through PgPostRepository.
//     Note: the HTML form routes (submit/edit/delete) write directly with Diesel
//     and therefore do not trigger broadcasts.  Broadcasts fire for REST API
//     mutations at /api/posts.  To broadcast from HTML routes too, call
//     state.broadcast().publish_oob(...) after each Diesel mutation.

use crate::hooks::PostHooks;
use crate::models::{
    NewPost, NewSubreddit, Post, PostDraftExt, Subreddit, UpdatePost, UpdateSubreddit,
};
use crate::schema::{posts, subreddits};

#[autumn_web::repository(Subreddit, api = "/api/subreddits")]
pub trait SubredditRepository {
    /// SELECT * FROM subreddits WHERE slug = $1
    fn find_by_slug(slug: String) -> Vec<Subreddit>;

    /// SELECT * FROM subreddits WHERE creator_id = $1
    fn find_by_creator_id(creator_id: i64) -> Vec<Subreddit>;
}

// `broadcasts = "posts"` wires every mutation that goes through PgPostRepository
// (save/update_by_id/delete_by_id) to publish an `hx-swap-oob` fragment on the
// "posts" channel.  Clients subscribing to `/posts/stream` receive live patches.
#[autumn_web::repository(Post, hooks = PostHooks, api = "/api/posts", broadcasts = "posts")]
pub trait PostRepository {
    /// SELECT * FROM posts WHERE slug = $1
    fn find_by_slug(slug: String) -> Vec<Post>;

    /// SELECT * FROM posts WHERE subreddit_id = $1
    fn find_by_subreddit_id(subreddit_id: i64) -> Vec<Post>;

    /// SELECT * FROM posts WHERE author_id = $1
    fn find_by_author_id(author_id: i64) -> Vec<Post>;
}

// LiveFragment renders a compact list-item fragment for each post.
// The macro uses this to build the hx-swap-oob payload when save/update/delete fires.
impl autumn_web::live::LiveFragment for Post {
    fn dom_id_for(id: i64) -> String {
        format!("post-{id}")
    }

    fn dom_id(&self) -> String {
        Self::dom_id_for(self.id)
    }

    fn render_fragment(&self) -> maud::Markup {
        maud::html! {
            li id=(self.dom_id()) class="live-post" {
                span class="post-score" { (self.score) " pts" }
                " "
                a href=(format!("/posts/{}", self.id))
                  class="post-title" { (self.title) }
            }
        }
    }

    fn insert_swap() -> autumn_web::htmx::OobSwap {
        autumn_web::htmx::OobSwap::Target(
            autumn_web::htmx::OobMethod::BeforeEnd,
            "#posts-list".to_string(),
        )
    }
}
