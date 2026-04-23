//! Standardized pagination primitives.
//!
//! Autumn ships with a page/size query-parameter contract, a metadata-rich
//! response wrapper, and a few helpers that make paginating a list endpoint
//! feel like a one-liner.
//!
//! # Quick start
//!
//! Paginating a handler takes three lines: run the count query, run the page
//! query, and wrap the result in a [`Page`].
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::pagination::{Page, PageRequest};
//! use diesel::prelude::*;
//! use diesel_async::RunQueryDsl;
//!
//! #[get("/api/posts")]
//! async fn list(page: PageRequest, mut db: Db) -> AutumnResult<Json<Page<Post>>> {
//!     let total: i64 = posts::table.count().get_result(&mut db).await?;
//!     let items: Vec<Post> = posts::table
//!         .limit(page.limit()).offset(page.offset())
//!         .select(Post::as_select())
//!         .load(&mut db).await?;
//!     Ok(Json(Page::new(items, total, &page)))
//! }
//! ```
//!
//! # Query contract
//!
//! Clients control pagination with two query parameters:
//!
//! | Parameter | Meaning | Default | Clamped to |
//! |-----------|---------|---------|------------|
//! | `page` | 1-based page index | `1` | `>= 1` |
//! | `size` | Items per page | [`DEFAULT_PAGE_SIZE`] | <code>1..=[`MAX_PAGE_SIZE`]</code> |
//!
//! Requests like `?size=0`, `?size=9999`, or `?page=0` are silently coerced
//! to the valid range rather than rejected — bad pagination parameters
//! should not 400.
//!
//! # Response shape
//!
//! [`Page<T>`] serializes as:
//!
//! ```json
//! {
//!   "content": [ ... ],
//!   "page": 1,
//!   "size": 20,
//!   "total_elements": 137,
//!   "total_pages": 7,
//!   "has_next": true,
//!   "has_previous": false
//! }
//! ```

use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use serde::{Deserialize, Serialize};

/// Default number of items per page when no `size` is provided.
pub const DEFAULT_PAGE_SIZE: u32 = 20;

/// Hard upper bound on `size` — prevents clients from requesting huge
/// pages that could OOM the server or overwhelm the database.
pub const MAX_PAGE_SIZE: u32 = 100;

// ── PageRequest ─────────────────────────────────────────────────────

/// Pagination parameters parsed from the query string.
///
/// Use as a handler extractor to receive `?page=N&size=M`. Both fields
/// are optional; missing values fall back to [`DEFAULT_PAGE_SIZE`] and
/// page `1`. Out-of-range values are clamped rather than rejected:
/// `page < 1` becomes `1`, `size` is clamped to <code>1..=[`MAX_PAGE_SIZE`]</code>.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::pagination::PageRequest;
///
/// #[get("/api/items")]
/// async fn list(page: PageRequest) -> String {
///     format!("page {} (limit {}, offset {})", page.page(), page.limit(), page.offset())
/// }
/// ```
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PageRequest {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    size: Option<u32>,
}

impl PageRequest {
    /// Construct a [`PageRequest`] explicitly. Values are clamped to the
    /// valid ranges defined by [`DEFAULT_PAGE_SIZE`] / [`MAX_PAGE_SIZE`].
    #[must_use]
    pub const fn new(page: u32, size: u32) -> Self {
        Self {
            page: Some(page),
            size: Some(size),
        }
    }

    /// Resolved 1-based page number. `0` or missing is coerced to `1`.
    #[must_use]
    pub const fn page(&self) -> u32 {
        match self.page {
            Some(p) if p >= 1 => p,
            _ => 1,
        }
    }

    /// Resolved page size, clamped to <code>1..=[`MAX_PAGE_SIZE`]</code>.
    #[must_use]
    pub const fn size(&self) -> u32 {
        match self.size {
            Some(0) | None => DEFAULT_PAGE_SIZE,
            Some(s) if s > MAX_PAGE_SIZE => MAX_PAGE_SIZE,
            Some(s) => s,
        }
    }

    /// `LIMIT` value for a Diesel or raw SQL query (`== size()`).
    #[must_use]
    pub const fn limit(&self) -> i64 {
        self.size() as i64
    }

    /// `OFFSET` value for a Diesel or raw SQL query.
    #[must_use]
    pub const fn offset(&self) -> i64 {
        ((self.page() - 1) as i64) * (self.size() as i64)
    }
}

impl<S> FromRequestParts<S> for PageRequest
where
    S: Send + Sync,
{
    type Rejection = <Query<Self> as FromRequestParts<S>>::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Missing query string (`/items` with no `?…`) is a valid request
        // for a PageRequest — fall back to defaults rather than rejecting.
        if parts.uri.query().is_none() {
            return Ok(Self::default());
        }
        Query::<Self>::from_request_parts(parts, state)
            .await
            .map(|Query(p)| p)
    }
}

// ── Page<T> ─────────────────────────────────────────────────────────

/// Paginated response wrapper with navigation metadata.
///
/// `Page` serializes to JSON for API responses and exposes the fields a
/// Maud template needs to render pager links (previous/next, page index,
/// total pages).
///
/// Construct one with [`Page::new`] after running your count + page
/// queries, or with [`Page::empty`] when you have no data to return.
///
/// # JSON shape
///
/// ```json
/// {
///   "content": [ /* T items */ ],
///   "page": 1,
///   "size": 20,
///   "total_elements": 137,
///   "total_pages": 7,
///   "has_next": true,
///   "has_previous": false
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    /// The items on this page.
    pub content: Vec<T>,
    /// Current 1-based page index.
    pub page: u32,
    /// Page size used to produce `content`.
    pub size: u32,
    /// Total number of items across every page.
    pub total_elements: u64,
    /// Total number of pages (`ceil(total_elements / size)`), minimum `1`.
    pub total_pages: u32,
    /// Whether there is a page after this one.
    pub has_next: bool,
    /// Whether there is a page before this one.
    pub has_previous: bool,
}

impl<T> Page<T> {
    /// Build a page from the materialized `items` and the total row
    /// count returned by the database.
    ///
    /// `total` is accepted as `i64` to match Diesel's
    /// `COUNT(*)` result type; values below zero are treated as zero.
    #[must_use]
    pub fn new(items: Vec<T>, total: i64, request: &PageRequest) -> Self {
        let size = request.size();
        let page = request.page();
        let total_elements = u64::try_from(total).unwrap_or(0);

        // ceil(total / size), minimum 1 so an empty result still
        // reports `total_pages = 1` — callers don't have to branch on
        // "no rows" when rendering a pager.
        let total_pages = if total_elements == 0 {
            1
        } else {
            // size() is always >= 1, so this division is safe.
            u32::try_from(total_elements.div_ceil(u64::from(size))).unwrap_or(u32::MAX)
        };

        Self {
            content: items,
            page,
            size,
            total_elements,
            total_pages,
            has_next: page < total_pages,
            has_previous: page > 1,
        }
    }

    /// Build an empty page using the caller's request parameters.
    ///
    /// Useful when a filter short-circuits before hitting the database.
    #[must_use]
    pub fn empty(request: &PageRequest) -> Self {
        Self::new(Vec::new(), 0, request)
    }

    /// Transform the content while preserving pagination metadata.
    ///
    /// Typical use: converting database rows into DTOs for JSON output
    /// without re-running the count query.
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> Page<U> {
        Page {
            content: self.content.into_iter().map(f).collect(),
            page: self.page,
            size: self.size,
            total_elements: self.total_elements,
            total_pages: self.total_pages,
            has_next: self.has_next,
            has_previous: self.has_previous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    // ── PageRequest coercion ────────────────────────────────────

    #[test]
    fn defaults_when_nothing_provided() {
        let r = PageRequest::default();
        assert_eq!(r.page(), 1);
        assert_eq!(r.size(), DEFAULT_PAGE_SIZE);
        assert_eq!(r.limit(), i64::from(DEFAULT_PAGE_SIZE));
        assert_eq!(r.offset(), 0);
    }

    #[test]
    fn page_zero_is_coerced_to_one() {
        let r = PageRequest::new(0, 10);
        assert_eq!(r.page(), 1);
        assert_eq!(r.offset(), 0);
    }

    #[test]
    fn size_is_clamped_to_max() {
        let r = PageRequest::new(1, 9_999);
        assert_eq!(r.size(), MAX_PAGE_SIZE);
        assert_eq!(r.limit(), i64::from(MAX_PAGE_SIZE));
    }

    #[test]
    fn size_zero_falls_back_to_default() {
        let r = PageRequest::new(3, 0);
        assert_eq!(r.size(), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn offset_matches_page_and_size() {
        let r = PageRequest::new(3, 25);
        assert_eq!(r.offset(), 50);
        assert_eq!(r.limit(), 25);
    }

    // ── Page metadata ──────────────────────────────────────────

    #[test]
    fn empty_page_has_one_total_page() {
        let page: Page<i32> = Page::empty(&PageRequest::default());
        assert_eq!(page.total_elements, 0);
        assert_eq!(page.total_pages, 1);
        assert!(!page.has_next);
        assert!(!page.has_previous);
    }

    #[test]
    fn metadata_reflects_middle_page() {
        let req = PageRequest::new(3, 20);
        let page = Page::new(vec![1_i32; 20], 137, &req);
        assert_eq!(page.page, 3);
        assert_eq!(page.size, 20);
        assert_eq!(page.total_elements, 137);
        assert_eq!(page.total_pages, 7); // ceil(137/20) == 7
        assert!(page.has_next);
        assert!(page.has_previous);
    }

    #[test]
    fn metadata_reflects_last_page() {
        let req = PageRequest::new(7, 20);
        let page = Page::new(vec![1_i32; 17], 137, &req);
        assert_eq!(page.total_pages, 7);
        assert!(!page.has_next);
        assert!(page.has_previous);
    }

    #[test]
    fn negative_total_is_treated_as_zero() {
        let page: Page<i32> = Page::new(vec![], -1, &PageRequest::default());
        assert_eq!(page.total_elements, 0);
        assert_eq!(page.total_pages, 1);
    }

    #[test]
    fn map_preserves_metadata() {
        let req = PageRequest::new(2, 10);
        let page = Page::new(vec![1_i32, 2, 3], 25, &req);
        let mapped = page.map(|n| n.to_string());
        assert_eq!(mapped.page, 2);
        assert_eq!(mapped.total_elements, 25);
        assert_eq!(mapped.total_pages, 3);
        assert_eq!(mapped.content, vec!["1", "2", "3"]);
    }

    // ── JSON serialization ─────────────────────────────────────

    #[test]
    fn page_serializes_to_expected_shape() {
        let req = PageRequest::new(2, 10);
        let page = Page::new(vec!["a", "b"], 25, &req);
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["page"], 2);
        assert_eq!(json["size"], 10);
        assert_eq!(json["total_elements"], 25);
        assert_eq!(json["total_pages"], 3);
        assert_eq!(json["has_next"], true);
        assert_eq!(json["has_previous"], true);
        assert_eq!(json["content"], serde_json::json!(["a", "b"]));
    }

    // ── Extractor tests ────────────────────────────────────────

    async fn echo(page: PageRequest) -> String {
        format!("{}:{}:{}", page.page(), page.size(), page.offset())
    }

    async fn fetch(uri: &str) -> (StatusCode, String) {
        let app = Router::new().route("/items", get(echo));
        let res = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn extractor_uses_defaults_when_query_missing() {
        let (status, body) = fetch("/items").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("1:{DEFAULT_PAGE_SIZE}:0"));
    }

    #[tokio::test]
    async fn extractor_parses_page_and_size() {
        let (status, body) = fetch("/items?page=4&size=25").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "4:25:75");
    }

    #[tokio::test]
    async fn extractor_clamps_size_over_max() {
        let (status, body) = fetch("/items?page=1&size=5000").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("1:{MAX_PAGE_SIZE}:0"));
    }

    #[tokio::test]
    async fn extractor_coerces_page_zero_to_one() {
        let (status, body) = fetch("/items?page=0&size=10").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1:10:0");
    }
}
