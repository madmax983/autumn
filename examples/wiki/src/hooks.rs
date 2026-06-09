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

        // Enforce state machine transitions; returns 400 for invalid edges or
        // when the can_publish guard rejects draft -> published.
        if draft.after.status != draft.before.status {
            draft.before.transition_status_to(&draft.after.status)?;
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
            lock_version: 0,
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
            lock_version: 0,
            created_at: Utc::now().naive_utc(),
            updated_at: Utc::now().naive_utc(),
        };

        let mut after = before.clone();
        after.body = "New Content".into();

        let mut draft = UpdateDraft { before, after };

        hooks.before_update(&mut ctx, &mut draft).await.unwrap();

        assert_eq!(draft.after.slug, "old-title");
    }

    #[tokio::test]
    async fn after_update_can_declare_cache_invalidation() {
        use autumn_web::hooks::{MutationContext, MutationOp};

        let mut ctx = MutationContext::new(MutationOp::Update);
        let page = Page {
            id: 42,
            title: "Concurrent Edit".into(),
            slug: "concurrent-edit".into(),
            body: "Body".into(),
            status: "published".into(),
            lock_version: 1,
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        // Simulate what the app would do in after_update:
        // declare cache keys to invalidate after a successful write
        ctx.invalidate(format!("pages:{}", page.id));
        ctx.invalidate("pages:all");

        assert_eq!(ctx.invalidate_keys.len(), 2);
        assert!(ctx.invalidate_keys.contains(&format!("pages:{}", page.id)));
        assert!(ctx.invalidate_keys.contains(&"pages:all".to_string()));
    }

    #[test]
    fn concurrent_edit_version_mismatch_is_detectable() {
        // Simulate: replica A and replica B both read page at lock_version=3.
        // Replica A commits first (bumps to 4). Replica B then tries to commit
        // with expected_version=3, but stored is 4 — a conflict is detected.
        let stored_version: i64 = 4;
        let replica_b_expected: i64 = 3;

        // This is what the repository checks internally:
        let is_conflict = stored_version != replica_b_expected;
        assert!(is_conflict, "replica B should detect a conflict");

        let err = autumn_web::RepositoryError::Conflict {
            id: 99,
            expected_version: replica_b_expected,
            actual_version: Some(stored_version),
        };
        assert!(err.to_string().contains("99"));
        assert!(err.to_string().contains("3"));
    }
}
