// ── Repository layer ────────────────────────────────────────────
//
// Manual Diesel queries using the Db extractor. The #[repository]
// macro will generate this boilerplate automatically once macro path
// resolution is fully stabilized for downstream crates.

use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use autumn_web::prelude::*;

use crate::models::{Bookmark, NewBookmark};
use crate::schema::bookmarks;

impl Bookmark {
    pub async fn all(db: &mut Db) -> AutumnResult<Vec<Self>> {
        Ok(bookmarks::table
            .order(bookmarks::created_at.desc())
            .select(Self::as_select())
            .load(&mut **db)
            .await?)
    }

    pub async fn find_by_tag(tag: &str, db: &mut Db) -> AutumnResult<Vec<Self>> {
        Ok(bookmarks::table
            .filter(bookmarks::tag.eq(tag))
            .order(bookmarks::created_at.desc())
            .select(Self::as_select())
            .load(&mut **db)
            .await?)
    }

    pub async fn create(new: &NewBookmark, db: &mut Db) -> AutumnResult<Self> {
        Ok(diesel::insert_into(bookmarks::table)
            .values(new)
            .returning(Self::as_returning())
            .get_result(&mut **db)
            .await?)
    }

    pub async fn delete(id: i32, db: &mut Db) -> AutumnResult<()> {
        diesel::delete(bookmarks::table.find(id))
            .execute(&mut **db)
            .await?;
        Ok(())
    }
}
