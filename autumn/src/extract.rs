//! Re-exports of Axum extractors for use in Autumn handlers.
//!
//! These are provided so users don't need `axum` as a direct dependency
//! for the most common extractor types.
//!
//! | Extractor | Purpose |
//! |-----------|---------|
//! | [`Form`] | Deserialize `application/x-www-form-urlencoded` request bodies |
//! | [`Json`] | Deserialize/serialize JSON request and response bodies |
//! | [`Path`] | Extract path parameters (e.g., `/users/{id}`) |
//! | [`Query`] | Deserialize URL query strings (e.g., `?page=2&limit=10`) |
//!
//! [`Json`] serves double duty -- it is both an extractor (parses JSON
//! request bodies) and a response type (serializes to JSON with
//! `Content-Type: application/json`).
//!
//! For the full set of Axum extractors, use
//! `autumn_web::reexports::axum::extract`.

/// Deserialize `application/x-www-form-urlencoded` request bodies.
///
/// Re-exported from [`axum::extract::Form`]. Commonly used with
/// HTML `<form>` submissions.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Login { username: String, password: String }
///
/// #[post("/login")]
/// async fn login(Form(input): Form<Login>) -> String {
///     format!("Welcome, {}!", input.username)
/// }
/// ```
pub use axum::extract::Form;

/// Deserialize and serialize JSON request/response bodies.
///
/// Re-exported from [`axum::extract::Json`]. As an extractor, parses the
/// request body. As a return type, serializes the value with
/// `Content-Type: application/json`.
///
/// Also available at the crate root as [`autumn_web::Json`](crate::Json).
pub use axum::extract::Json;

/// Extract typed path parameters from the URL.
///
/// Re-exported from [`axum::extract::Path`]. Use with route patterns
/// like `/users/{id}`.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::extract::Path;
///
/// #[get("/users/{id}")]
/// async fn get_user(Path(id): Path<i32>) -> String {
///     format!("User {id}")
/// }
/// ```
pub use axum::extract::Path;

/// Deserialize URL query string parameters.
///
/// Re-exported from [`axum::extract::Query`]. Parses the query string
/// into a typed struct.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::extract::Query;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Pagination { page: u32, limit: u32 }
///
/// #[get("/items")]
/// async fn list_items(Query(params): Query<Pagination>) -> String {
///     format!("Page {} (limit {})", params.page, params.limit)
/// }
/// ```
pub use axum::extract::Query;

pub use axum::extract::State;
