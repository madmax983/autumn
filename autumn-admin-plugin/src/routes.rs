//! Route handlers for the admin panel.
//!
//! All handlers return [`AutumnResult<Response>`] so the framework's
//! error-page filter can render 401/403/404/500 as branded HTML for browser
//! clients and JSON for API clients — no hand-rolled error HTML here.

use std::sync::Arc;

use autumn_web::flash::Flash;
use autumn_web::prelude::HxResponseExt;
use autumn_web::security::CsrfToken;
use autumn_web::{AppState, AutumnError, AutumnResult};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;
use futures::future::join_all;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::check_role;
use crate::registry::AdminRegistry;
use crate::templates;
use crate::traits::{AdminModel, ListParams, SortDirection, record_id};

// ── Router construction ─────────────────────────────────────────────

pub fn admin_router(
    registry: Arc<AdminRegistry>,
    prefix: &str,
    actuator_prefix: String,
    require_role: Option<String>,
) -> axum::Router<AppState> {
    let router = axum::Router::new()
        // Dashboard
        .route("/", routing::get(dashboard))
        // Model routes (dynamic dispatch via slug)
        .route("/{slug}", routing::get(model_list).post(model_create))
        .route("/{slug}/new", routing::get(model_new_form))
        .route(
            "/{slug}/{id}",
            routing::get(model_detail)
                .post(model_update)
                .delete(model_delete),
        )
        .route("/{slug}/{id}/edit", routing::get(model_edit_form))
        .layer(axum::Extension(AdminPrefix(prefix.to_owned())))
        .layer(axum::Extension(ActuatorPrefix(actuator_prefix)))
        .layer(axum::Extension(registry));

    match require_role {
        Some(role) => router.layer(from_fn(move |req, next| {
            check_role(role.clone(), req, next)
        })),
        None => router,
    }
}

/// Typed Extension carrying the admin URL prefix so handlers can build links.
#[derive(Clone)]
struct AdminPrefix(String);

/// Typed Extension carrying the actuator URL prefix (the value of
/// `config.actuator.prefix`), used for dashboard links and HTMX polling.
#[derive(Clone)]
struct ActuatorPrefix(String);

// ── Query params ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    dir: SortDirection,
}

const fn default_page() -> u64 {
    1
}

// ── Shared resolution ───────────────────────────────────────────────

/// Resolve the DB pool + model for a slug, translating missing state into
/// `AutumnError` so handlers can use `?`.
fn resolve<'r>(
    state: &AppState,
    registry: &'r AdminRegistry,
    slug: &str,
) -> AutumnResult<(Pool<AsyncPgConnection>, &'r dyn AdminModel)> {
    // `Pool` is Arc-backed inside deadpool; cloning is cheap.
    let pool = state
        .pool()
        .cloned()
        .ok_or_else(|| AutumnError::service_unavailable_msg("No database pool configured"))?;
    let model = registry
        .get(slug)
        .ok_or_else(|| AutumnError::not_found_msg(format!("Model '{slug}' not found")))?;
    Ok((pool, model))
}

fn to_autumn(err: impl std::fmt::Display) -> AutumnError {
    AutumnError::internal_server_error_msg(err.to_string())
}

/// Render a Maud `Markup` into an `Html` response.
fn render(markup: maud::Markup) -> Response {
    Html(markup.into_string()).into_response()
}

// ── Handlers ────────────────────────────────────────────────────────

/// `GET /admin` — Dashboard with model counts.
async fn dashboard(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    csrf: CsrfToken,
    flash: Flash,
) -> AutumnResult<Response> {
    let pool = state
        .pool()
        .cloned()
        .ok_or_else(|| AutumnError::service_unavailable_msg("No database pool configured"))?;

    let futures: Vec<_> = registry
        .iter()
        .map(|(slug, model)| {
            let pool = pool.clone();
            async move {
                let count = model.count(&pool).await.unwrap_or(0);
                (slug, model.display_name_plural(), count)
            }
        })
        .collect();
    let counts = join_all(futures).await;
    let messages = flash.consume().await;

    Ok(render(templates::dashboard_page(
        &registry,
        &counts,
        &messages,
        csrf.token(),
        &prefix,
        &actuator_prefix,
    )))
}

/// `GET /admin/{slug}` — Model list view.
#[allow(clippy::too_many_arguments)]
async fn model_list(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    Path(slug): Path<String>,
    Query(query): Query<ListQuery>,
    csrf: CsrfToken,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let ListQuery { page, q, sort, dir } = query;
    let page = page.max(1);
    let per_page = model.per_page();

    let params = ListParams {
        page,
        per_page,
        search: (!q.is_empty()).then(|| q.clone()),
        sort_by: sort.clone(),
        sort_dir: dir,
        filters: vec![],
    };

    let result = model.list(&pool, params).await.map_err(to_autumn)?;

    let fields = model.fields();
    let messages = flash.consume().await;
    Ok(render(templates::model_list_page(
        &registry,
        &slug,
        model.display_name_plural(),
        &fields,
        &result,
        &q,
        sort.as_deref(),
        dir,
        &messages,
        csrf.token(),
        &prefix,
        &actuator_prefix,
    )))
}

/// `GET /admin/{slug}/new` — Create form.
async fn model_new_form(
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    Path(slug): Path<String>,
    csrf: CsrfToken,
    flash: Flash,
) -> AutumnResult<Response> {
    let model = registry
        .get(&slug)
        .ok_or_else(|| AutumnError::not_found_msg(format!("Model '{slug}' not found")))?;

    let fields = model.fields();
    let messages = flash.consume().await;
    Ok(render(templates::model_form_page(
        &registry,
        &slug,
        model.display_name(),
        model.display_name_plural(),
        &fields,
        None,
        &messages,
        csrf.token(),
        &prefix,
        &actuator_prefix,
    )))
}

/// `POST /admin/{slug}` — Create a record.
async fn model_create(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    Path(slug): Path<String>,
    flash: Flash,
    axum::extract::Form(form_data): axum::extract::Form<Value>,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let record = model
        .create(&pool, strip_meta_fields(form_data))
        .await
        .map_err(|e| AutumnError::bad_request_msg(format!("Create failed: {e}")))?;
    flash
        .success(format!("{} created.", model.display_name()))
        .await;
    Ok(Redirect::to(&format!("{prefix}/{slug}/{}", record_id(&record))).into_response())
}

/// `GET /admin/{slug}/{id}` — Detail view.
async fn model_detail(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    Path((slug, id)): Path<(String, i64)>,
    csrf: CsrfToken,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let record = model
        .get(&pool, id)
        .await
        .map_err(to_autumn)?
        .ok_or_else(|| {
            AutumnError::not_found_msg(format!("{} #{id} not found", model.display_name()))
        })?;

    let display = model.record_display(&record);
    let fields = model.fields();
    let messages = flash.consume().await;
    Ok(render(templates::model_detail_page(
        &registry,
        &slug,
        model.display_name(),
        model.display_name_plural(),
        &fields,
        &record,
        &display,
        &messages,
        csrf.token(),
        &prefix,
        &actuator_prefix,
    )))
}

/// `GET /admin/{slug}/{id}/edit` — Edit form.
async fn model_edit_form(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    Path((slug, id)): Path<(String, i64)>,
    csrf: CsrfToken,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let record = model
        .get(&pool, id)
        .await
        .map_err(to_autumn)?
        .ok_or_else(|| {
            AutumnError::not_found_msg(format!("{} #{id} not found", model.display_name()))
        })?;

    let fields = model.fields();
    let messages = flash.consume().await;
    Ok(render(templates::model_form_page(
        &registry,
        &slug,
        model.display_name(),
        model.display_name_plural(),
        &fields,
        Some(&record),
        &messages,
        csrf.token(),
        &prefix,
        &actuator_prefix,
    )))
}

/// `POST /admin/{slug}/{id}` — Update a record.
async fn model_update(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    Path((slug, id)): Path<(String, i64)>,
    flash: Flash,
    axum::extract::Form(form_data): axum::extract::Form<Value>,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    model
        .update(&pool, id, strip_meta_fields(form_data))
        .await
        .map_err(|e| AutumnError::bad_request_msg(format!("Update failed: {e}")))?;
    flash
        .success(format!("{} #{id} updated.", model.display_name()))
        .await;
    Ok(Redirect::to(&format!("{prefix}/{slug}/{id}")).into_response())
}

/// `DELETE /admin/{slug}/{id}` — Delete a record.
///
/// Called from the detail view's `hx-delete` button. Returns an empty 200
/// body with `HX-Redirect` so htmx performs a full-page navigation to the
/// list view (updating `window.location`), rather than swapping the list
/// HTML into the stale detail page.
async fn model_delete(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    Path((slug, id)): Path<(String, i64)>,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    model
        .delete(&pool, id)
        .await
        .map_err(|e| AutumnError::bad_request_msg(format!("Delete failed: {e}")))?;
    flash
        .success(format!("{} #{id} deleted.", model.display_name()))
        .await;
    Ok(StatusCode::OK.hx_redirect(&format!("{prefix}/{slug}")))
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Remove form-internal fields (`_csrf`, anything else starting with `_`) and
/// any password field whose value is blank (so "leave blank to keep current"
/// works on edit forms without wiping the existing hash).
fn strip_meta_fields(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.retain(|k, v| {
            if k.starts_with('_') {
                return false;
            }
            // Discard blank password fields so they don't overwrite stored hashes.
            // We can't know from the raw form data which fields are passwords, so
            // the template strips empty password inputs client-side before submit.
            // This retain runs anyway as a server-side safety net for string-typed
            // values that arrive empty.
            !matches!(v, Value::String(s) if s.is_empty() && is_likely_password(k))
        });
    }
    data
}

fn is_likely_password(field: &str) -> bool {
    let lower = field.to_ascii_lowercase();
    lower.contains("password") || lower.contains("passwd") || lower == "pwd"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_meta_removes_csrf_and_underscore_fields() {
        let input = json!({"name": "x", "_csrf": "t", "_foo": 1});
        let out = strip_meta_fields(input);
        assert_eq!(out, json!({"name": "x"}));
    }

    #[test]
    fn strip_meta_drops_blank_password_but_keeps_filled() {
        let input = json!({"password": "", "other": "y"});
        assert_eq!(strip_meta_fields(input), json!({"other": "y"}));

        let input = json!({"password": "hunter2", "other": "y"});
        assert_eq!(
            strip_meta_fields(input),
            json!({"password": "hunter2", "other": "y"})
        );
    }

    #[test]
    fn strip_meta_preserves_blank_non_password_fields() {
        let input = json!({"name": "", "bio": ""});
        assert_eq!(strip_meta_fields(input), json!({"name": "", "bio": ""}));
    }

    #[test]
    fn is_likely_password_detects_variants() {
        assert!(is_likely_password("password"));
        assert!(is_likely_password("Password"));
        assert!(is_likely_password("new_password"));
        assert!(is_likely_password("passwd"));
        assert!(is_likely_password("pwd"));
        assert!(!is_likely_password("name"));
        assert!(!is_likely_password("email"));
    }
}
