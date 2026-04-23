//! Integration tests for the pagination extractor and response wrapper.
//!
//! Verifies the full round-trip through Axum: query-string parsing,
//! clamping of out-of-range parameters, and JSON serialization of
//! [`Page`] responses.

use autumn_web::extract::Json;
use autumn_web::pagination::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, Page, PageRequest};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Item {
    id: i64,
    name: String,
}

fn seed(count: i64) -> Vec<Item> {
    (1..=count)
        .map(|id| Item {
            id,
            name: format!("item-{id}"),
        })
        .collect()
}

fn app() -> Router {
    async fn list(page: PageRequest) -> Json<Page<Item>> {
        // Simulate "select the current page from a 137-row table".
        let all = seed(137);
        let total = i64::try_from(all.len()).unwrap_or(i64::MAX);
        let start = usize::try_from(page.offset()).unwrap_or(0);
        let end = start.saturating_add(page.size() as usize).min(all.len());
        let items = if start < all.len() {
            all[start..end].to_vec()
        } else {
            Vec::new()
        };
        Json(Page::new(items, total, &page))
    }
    Router::new().route("/items", get(list))
}

async fn fetch_json(uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}

#[tokio::test]
async fn first_page_reports_defaults_and_has_next() {
    let (status, body) = fetch_json("/items").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["page"], 1);
    assert_eq!(body["size"], DEFAULT_PAGE_SIZE);
    assert_eq!(body["total_elements"], 137);
    assert_eq!(body["total_pages"], 7);
    assert_eq!(body["has_previous"], false);
    assert_eq!(body["has_next"], true);
    assert_eq!(body["content"].as_array().unwrap().len(), 20);
    assert_eq!(body["content"][0]["id"], 1);
}

#[tokio::test]
async fn middle_page_has_next_and_previous() {
    let (status, body) = fetch_json("/items?page=3&size=20").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["page"], 3);
    assert_eq!(body["has_previous"], true);
    assert_eq!(body["has_next"], true);
    assert_eq!(body["content"][0]["id"], 41);
}

#[tokio::test]
async fn last_page_is_partially_full() {
    let (status, body) = fetch_json("/items?page=7&size=20").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["page"], 7);
    assert_eq!(body["has_next"], false);
    assert_eq!(body["has_previous"], true);
    // 137 rows / 20 per page → last page has 17 items
    assert_eq!(body["content"].as_array().unwrap().len(), 17);
}

#[tokio::test]
async fn past_end_yields_empty_content() {
    let (status, body) = fetch_json("/items?page=99&size=20").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"].as_array().unwrap().len(), 0);
    assert_eq!(body["has_next"], false);
    assert_eq!(body["has_previous"], true);
}

#[tokio::test]
async fn size_exceeding_max_is_clamped() {
    let (status, body) = fetch_json("/items?page=1&size=500").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["size"], MAX_PAGE_SIZE);
    assert_eq!(
        body["content"].as_array().unwrap().len(),
        MAX_PAGE_SIZE as usize
    );
}

#[tokio::test]
async fn page_zero_is_coerced_to_first_page() {
    let (status, body) = fetch_json("/items?page=0&size=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["page"], 1);
    assert_eq!(body["content"][0]["id"], 1);
}

#[tokio::test]
async fn size_zero_falls_back_to_default() {
    let (status, body) = fetch_json("/items?page=1&size=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["size"], DEFAULT_PAGE_SIZE);
}
