use crate::hooks::PageHooks;
use crate::models::{NewPage, Page, PageDraftExt, UpdatePage};
use crate::schema::pages;

#[autumn_web::repository(Page, hooks = PageHooks)]
pub trait PageRepository {
    fn find_by_slug(slug: String) -> Vec<Page>;
    fn find_by_status(status: String) -> Vec<Page>;
}
