use autumn_web::AutumnResult;
use autumn_web::hooks::{MutationContext, MutationHooks, UpdateDraft};

use crate::models::{NewPage, Page, UpdatePage};
use crate::slugify::slugify;

#[derive(Clone, Default)]
pub struct PageHooks;

impl MutationHooks for PageHooks {
    type Model = Page;
    type NewModel = NewPage;
    type UpdateModel = UpdatePage;

    async fn before_create(
        &self,
        _ctx: &mut MutationContext,
        new: &mut NewPage,
    ) -> AutumnResult<()> {
        // Auto-generate slug from title
        new.slug = slugify(&new.title);

        // Default status to "draft" if empty
        if new.status.trim().is_empty() {
            new.status = "draft".into();
        }

        Ok(())
    }

    async fn before_update(
        &self,
        _ctx: &mut MutationContext,
        draft: &mut UpdateDraft<Page>,
    ) -> AutumnResult<()> {
        // Re-slug if title changed
        if draft.after.title != draft.before.title {
            draft.after.slug = slugify(&draft.after.title);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::hooks::MutationOp;
    use chrono::Utc;

    #[tokio::test]
    async fn test_before_update_updates_slug_if_title_changes() {
        let hooks = PageHooks;
        let mut ctx = MutationContext::new(MutationOp::Update);
        let before = Page {
            id: 1,
            title: "Old Title".into(),
            slug: "old-title".into(),
            body: "Old Content".into(),
            status: "published".into(),
            created_at: Utc::now().naive_utc(),
            updated_at: Utc::now().naive_utc(),
        };

        let mut after = before.clone();
        after.title = "New Title".into();

        let mut draft = UpdateDraft { before, after };

        hooks.before_update(&mut ctx, &mut draft).await.unwrap();

        assert_eq!(draft.after.slug, "new-title");
    }

    #[tokio::test]
    async fn test_before_update_preserves_slug_if_title_unchanged() {
        let hooks = PageHooks;
        let mut ctx = MutationContext::new(MutationOp::Update);
        let before = Page {
            id: 1,
            title: "Old Title".into(),
            slug: "old-title".into(),
            body: "Old Content".into(),
            status: "published".into(),
            created_at: Utc::now().naive_utc(),
            updated_at: Utc::now().naive_utc(),
        };

        let mut after = before.clone();
        after.body = "New Content".into();

        let mut draft = UpdateDraft { before, after };

        hooks.before_update(&mut ctx, &mut draft).await.unwrap();

        assert_eq!(draft.after.slug, "old-title");
    }
}
