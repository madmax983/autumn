use autumn_web::AutumnResult;
use autumn_web::hooks::{MutationContext, MutationHooks, UpdateDraft};
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;

use crate::models::{NewPage, NewRevision, Page, UpdatePage};
use crate::schema::revisions;
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

    async fn after_create(
        &self,
        ctx: &MutationContext,
        record: &Page,
        conn: &mut AsyncPgConnection,
    ) -> AutumnResult<()> {
        // Transactional revision audit — runs inside the same tx as the INSERT
        let rev = NewRevision {
            page_id: record.id,
            op: "create".into(),
            title: record.title.clone(),
            body: record.body.clone(),
            status: record.status.clone(),
            changed_by: ctx.actor.clone(),
            summary: Some("Page created".into()),
        };

        diesel::insert_into(revisions::table)
            .values(&rev)
            .execute(conn)
            .await?;

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

    async fn after_update(
        &self,
        ctx: &MutationContext,
        record: &Page,
        conn: &mut AsyncPgConnection,
    ) -> AutumnResult<()> {
        let rev = NewRevision {
            page_id: record.id,
            op: "update".into(),
            title: record.title.clone(),
            body: record.body.clone(),
            status: record.status.clone(),
            changed_by: ctx.actor.clone(),
            summary: Some("Page updated".into()),
        };

        diesel::insert_into(revisions::table)
            .values(&rev)
            .execute(conn)
            .await?;

        Ok(())
    }
}
