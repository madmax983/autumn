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
use axum::http::{StatusCode, header};
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
use crate::traits::{AdminField, AdminFieldKind, AdminModel, ListParams, SortDirection, record_id};

/// Plugin-owned JS served at `{prefix}/static/admin.js`. External file
/// (not inline) so it works under the default CSP `script-src 'self'`.
const ADMIN_JS: &str = include_str!("admin.js");

/// Route (relative to the plugin prefix) where [`ADMIN_JS`] is served.
pub const ADMIN_JS_PATH: &str = "/static/admin.js";

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
        .route(ADMIN_JS_PATH, routing::get(serve_admin_js))
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

/// Serve the plugin's static JS with long-cache headers.
async fn serve_admin_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        ADMIN_JS,
    )
        .into_response()
}

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

    let fields = model.fields();
    let record = model
        .create(&pool, strip_meta_fields(form_data, &fields))
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

    let fields = model.fields();
    model
        .update(&pool, id, strip_meta_fields(form_data, &fields))
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
///
/// Uses [`AdminFieldKind::Password`] from the model's declared field metadata
/// — not a name heuristic — so custom password fields (`secret`, `api_key`,
/// whatever the admin chose) are all handled correctly.
fn strip_meta_fields(mut data: Value, fields: &[AdminField]) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.retain(|k, v| {
            if k.starts_with('_') {
                return false;
            }
            // Drop blank string values for fields the model declared as passwords,
            // so admins editing unrelated fields don't overwrite stored hashes.
            let is_declared_password = fields
                .iter()
                .any(|f| f.name == k && matches!(f.kind, AdminFieldKind::Password));
            !matches!(v, Value::String(s) if s.is_empty() && is_declared_password)
        });
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(specs: &[(&'static str, AdminFieldKind)]) -> Vec<AdminField> {
        specs
            .iter()
            .cloned()
            .map(|(name, kind)| AdminField::new(name, kind))
            .collect()
    }

    #[test]
    fn strip_meta_removes_csrf_and_underscore_fields() {
        let input = json!({"name": "x", "_csrf": "t", "_foo": 1});
        let out = strip_meta_fields(input, &fields(&[("name", AdminFieldKind::Text)]));
        assert_eq!(out, json!({"name": "x"}));
    }

    #[test]
    fn strip_meta_drops_blank_password_by_declared_kind() {
        let fields = fields(&[
            ("password", AdminFieldKind::Password),
            ("other", AdminFieldKind::Text),
        ]);
        let out = strip_meta_fields(json!({"password": "", "other": "y"}), &fields);
        assert_eq!(out, json!({"other": "y"}));

        let out = strip_meta_fields(json!({"password": "hunter2", "other": "y"}), &fields);
        assert_eq!(out, json!({"password": "hunter2", "other": "y"}));
    }

    #[test]
    fn strip_meta_drops_blank_custom_named_password() {
        // Regression: the old name-heuristic version missed this.
        // A field called "secret" declared as Password must still be stripped.
        let fields = fields(&[("secret", AdminFieldKind::Password)]);
        let out = strip_meta_fields(json!({"secret": ""}), &fields);
        assert_eq!(out, json!({}));
    }

    #[test]
    fn strip_meta_preserves_blank_non_password_fields() {
        let fields = fields(&[
            ("name", AdminFieldKind::Text),
            ("bio", AdminFieldKind::TextArea),
        ]);
        let out = strip_meta_fields(json!({"name": "", "bio": ""}), &fields);
        assert_eq!(out, json!({"name": "", "bio": ""}));
    }

    #[test]
    fn strip_meta_keeps_field_named_password_if_not_declared_as_such() {
        // If the model exposes a Text field literally named "password" (weird
        // but legal), we should NOT drop the empty string — the model gets to
        // decide. Only `AdminFieldKind::Password` triggers the strip.
        let fields = fields(&[("password", AdminFieldKind::Text)]);
        let out = strip_meta_fields(json!({"password": ""}), &fields);
        assert_eq!(out, json!({"password": ""}));
    }
}
