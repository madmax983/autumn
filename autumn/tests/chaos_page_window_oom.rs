use autumn_web::pagination::Page;
use autumn_web::ui::pagination::*;

#[test]
fn havoc_page_window_oom() {
    let opts = PagerOptions::new("/test").window(2_000_000_000);
    let page: Page<u32> = Page::from_raw(2_000_000_000, 10, 4_000_000_000_000, 400_000_000);
    let _ = pagination_nav(&page, &opts);
}
