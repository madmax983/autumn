use autumn_web::AutumnResult;
use autumn_web::hooks::{MutationContext, MutationHooks, UpdateDraft};
use diesel_async::AsyncPgConnection;

use crate::models::{NewPost, Post, UpdatePost};
use crate::slugify::slugify;

/// Mutation hooks for posts — auto-generate slug from title on
/// create and re-slug on title change during update.
#[derive(Clone, Default)]
pub struct PostHooks;

impl MutationHooks for PostHooks {
    type Model = Post;
    type NewModel = NewPost;
    type UpdateModel = UpdatePost;

    async fn before_create(
        &self,
        _ctx: &mut MutationContext,
        new: &mut NewPost,
    ) -> AutumnResult<()> {
        // Auto-generate slug from title
        new.slug = slugify(&new.title);
        Ok(())
    }

    async fn after_create(
        &self,
        _ctx: &MutationContext,
        record: &Post,
        _conn: &mut AsyncPgConnection,
    ) -> AutumnResult<()> {
        tracing::info!(
            post_id = record.id,
            slug = %record.slug,
            "New post created in r/{}",
            record.subreddit_id
        );
        Ok(())
    }

    async fn before_update(
        &self,
        _ctx: &mut MutationContext,
        draft: &mut UpdateDraft<Post>,
    ) -> AutumnResult<()> {
        // Re-slug if title changed
        if draft.after.title != draft.before.title {
            draft.after.slug = slugify(&draft.after.title);
        }
        Ok(())
    }
}
