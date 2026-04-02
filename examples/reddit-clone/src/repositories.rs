// ── v0.2 Feature: #[repository] macro ───────────────────────────
//
// Generates PgSubredditRepository and PgPostRepository with:
//   - Auto-generated CRUD methods (find_by_id, find_all, save, etc.)
//   - Derived queries parsed from method signatures
//   - FromRequestParts extractor (use as handler parameter)
//   - Optional REST API handler generation with `api = "..."`
//   - Optional mutation hooks with `hooks = ...`

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

#[autumn_web::repository(Post, hooks = PostHooks, api = "/api/posts")]
pub trait PostRepository {
    /// SELECT * FROM posts WHERE slug = $1
    fn find_by_slug(slug: String) -> Vec<Post>;

    /// SELECT * FROM posts WHERE subreddit_id = $1
    fn find_by_subreddit_id(subreddit_id: i64) -> Vec<Post>;

    /// SELECT * FROM posts WHERE author_id = $1
    fn find_by_author_id(author_id: i64) -> Vec<Post>;
}
