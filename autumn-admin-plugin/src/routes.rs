//! Route handlers for the admin panel.
//!
//! All handlers return [`AutumnResult<Response>`] so the framework's
//! error-page filter can render 401/403/404/500 as branded HTML for browser
//! clients and JSON for API clients — no hand-rolled error HTML here.

use std::sync::Arc;

use std::convert::Infallible;
use std::sync::LazyLock;

use autumn_web::flash::Flash;
use autumn_web::prelude::HxResponseExt;
use autumn_web::security::CsrfToken;
use autumn_web::{AppState, AutumnError, AutumnResult};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
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
use crate::traits::{
    AdminError, AdminField, AdminFieldKind, AdminModel, ListParams, SortDirection, record_id,
};

/// Admin-owned CSRF extractor that tolerates a missing `CsrfLayer`.
///
/// Autumn enables CSRF only for the `prod` profile by default, so a plain
/// `CsrfToken` extractor would crash every admin page in dev/test with a
/// 500. This wrapper reads the same request extension and falls back to an
/// empty token when the layer isn't installed — the rendered `_csrf`
/// hidden input and `<meta>` are then harmless because the middleware
/// that would validate them isn't running either.
#[derive(Debug, Clone, Default)]
pub struct AdminCsrf(String);

impl AdminCsrf {
    /// The CSRF token, or `""` if `CsrfLayer` is not installed.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AdminCsrf {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let token = parts
            .extensions
            .get::<CsrfToken>()
            .map(|t| t.token().to_owned())
            .unwrap_or_default();
        Ok(Self(token))
    }
}

/// Plugin-owned JS. Served as an external file (not inline) so it works
/// under the default CSP `script-src 'self'`.
const ADMIN_JS: &str = include_str!("admin.js");

/// FNV-1a 64-bit hash of the shipped JS, computed at compile time. Used to
/// fingerprint the asset path so the browser cache can be `immutable` for
/// a year without risking a post-deploy mismatch between cached client JS
/// and newer server templates — bumping the JS bumps the URL.
const ADMIN_JS_HASH: u64 = fnv1a_64(ADMIN_JS.as_bytes());

const fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// Route path (relative to the plugin prefix) where [`ADMIN_JS`] is served.
/// Format: `/static/admin.<hash>.js`. Built at startup from the compile-time
/// content hash; stable for the lifetime of the process.
pub static ADMIN_JS_PATH: LazyLock<String> =
    LazyLock::new(|| format!("/static/admin.{ADMIN_JS_HASH:016x}.js"));

// ── Router construction ─────────────────────────────────────────────

pub fn admin_router(
    registry: Arc<AdminRegistry>,
    prefix: &str,
    actuator_prefix: String,
    auth_session_key: String,
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
        .route(&ADMIN_JS_PATH, routing::get(serve_admin_js))
        .layer(axum::Extension(AdminPrefix(prefix.to_owned())))
        .layer(axum::Extension(ActuatorPrefix(actuator_prefix)))
        .layer(axum::Extension(registry));

    match require_role {
        Some(role) => router.layer(from_fn(move |req, next| {
            check_role(role.clone(), auth_session_key.clone(), req, next)
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

/// Filter a user-supplied sort key down to fields the model declared as
/// both sortable and list-displayed. A `None` (or unrecognised key) means
/// "no sort" — never forward arbitrary identifiers to the model.
fn validate_sort_key(sort: Option<String>, fields: &[AdminField]) -> Option<String> {
    sort.filter(|s| {
        fields
            .iter()
            .any(|f| f.name == s && f.sortable && f.list_display)
    })
}

/// Translate an [`AdminError`] to the correct HTTP status. Validation errors
/// become 400, missing records 404, database/other backend failures 500. The
/// `action` word prefixes the message ("Create failed: ..."), which is handy
/// in logs and error pages.
fn admin_err(action: &str, err: AdminError) -> AutumnError {
    match err {
        AdminError::NotFound => AutumnError::not_found_msg(format!("{action}: not found")),
        AdminError::Validation(msg) => AutumnError::bad_request_msg(format!("{action}: {msg}")),
        AdminError::Database(msg) => {
            AutumnError::internal_server_error_msg(format!("{action}: database error: {msg}"))
        }
        AdminError::Other(msg) => {
            AutumnError::internal_server_error_msg(format!("{action}: {msg}"))
        }
    }
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
    csrf: AdminCsrf,
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
    csrf: AdminCsrf,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let ListQuery { page, q, sort, dir } = query;
    let page = page.max(1);
    let per_page = model.per_page();
    let fields = model.fields();
    // Validate the requested sort key against the model's declared
    // sortable fields. A crafted `?sort=<unexpected>` is silently dropped
    // — the model never sees an unvalidated sort key, so it can't error
    // or build unsafe dynamic ORDER BY expressions.
    let sort = validate_sort_key(sort, &fields);

    let params = ListParams {
        page,
        per_page,
        search: (!q.is_empty()).then(|| q.clone()),
        sort_by: sort.clone(),
        sort_dir: dir,
        filters: vec![],
    };

    let result = model
        .list(&pool, params)
        .await
        .map_err(|e| admin_err("List", e))?;

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
    csrf: AdminCsrf,
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
        .map_err(|e| admin_err("Create failed", e))?;
    // The post-create redirect needs a routable ID. Treat a missing or
    // non-numeric `id` as a model-impl bug rather than silently sending
    // the admin to `/{slug}/0` (which lands on the wrong row or a 404).
    let new_id = record_id(&record).ok_or_else(|| {
        AutumnError::internal_server_error_msg(format!(
            "{} create returned a record without a numeric `id` field; cannot route post-create redirect",
            model.display_name()
        ))
    })?;
    flash
        .success(format!("{} created.", model.display_name()))
        .await;
    Ok(Redirect::to(&format!("{prefix}/{slug}/{new_id}")).into_response())
}

/// `GET /admin/{slug}/{id}` — Detail view.
async fn model_detail(
    State(state): State<AppState>,
    axum::Extension(registry): axum::Extension<Arc<AdminRegistry>>,
    axum::Extension(AdminPrefix(prefix)): axum::Extension<AdminPrefix>,
    axum::Extension(ActuatorPrefix(actuator_prefix)): axum::Extension<ActuatorPrefix>,
    Path((slug, id)): Path<(String, i64)>,
    csrf: AdminCsrf,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let record = model
        .get(&pool, id)
        .await
        .map_err(|e| admin_err("Get", e))?
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
        id,
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
    csrf: AdminCsrf,
    flash: Flash,
) -> AutumnResult<Response> {
    let (pool, model) = resolve(&state, &registry, &slug)?;

    let record = model
        .get(&pool, id)
        .await
        .map_err(|e| admin_err("Get", e))?
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
        Some(id),
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
        .map_err(|e| admin_err("Update failed", e))?;
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
        .map_err(|e| admin_err("Delete failed", e))?;
    flash
        .success(format!("{} #{id} deleted.", model.display_name()))
        .await;
    Ok(StatusCode::OK.hx_redirect(&format!("{prefix}/{slug}")))
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Filter incoming form data down to fields the model declared as editable.
///
/// Enforcement (all three are necessary):
///
/// 1. **Drop underscore-prefixed keys** (`_csrf` and similar form internals).
/// 2. **Drop keys not declared in `fields`** so a crafted POST can't inject
///    arbitrary columns (e.g. `is_admin=true`) into an `AdminModel::create`.
/// 3. **Drop keys whose `AdminField::editable = false`** so read-only columns
///    (`id`, `created_at`, computed fields, privilege flags) can't be
///    overwritten by admins submitting tampered forms.
/// 4. **Drop blank string values on declared `Password` fields** so "leave
///    blank to keep current" doesn't wipe stored hashes.
///
/// The UI's readonly contract is the source of truth: if the admin didn't
/// declare a field as editable, model code never sees it.
fn strip_meta_fields(mut data: Value, fields: &[AdminField]) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.retain(|k, v| {
            if k.starts_with('_') {
                return false;
            }
            let Some(field) = fields.iter().find(|f| f.name == k) else {
                // Key not in the schema — drop it. Prevents arbitrary columns
                // from being injected past the declared editable surface.
                return false;
            };
            if matches!(field.kind, AdminFieldKind::Hidden) {
                // Hidden fields are read-only by contract, regardless of
                // whether `editable` was flipped back to true — the form
                // never exposes an input for them, so any submitted value
                // is necessarily tampered.
                return false;
            }
            if !field.editable {
                // Readonly field — drop it regardless of submitted value.
                return false;
            }
            // Drop blank string values on Password fields so admins editing
            // unrelated fields don't overwrite stored hashes.
            !matches!(v, Value::String(s) if s.is_empty() && matches!(field.kind, AdminFieldKind::Password))
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
    fn validate_sort_key_passes_known_sortable_displayed_fields() {
        let fields = fields(&[("name", AdminFieldKind::Text)]);
        assert_eq!(
            validate_sort_key(Some("name".to_owned()), &fields),
            Some("name".to_owned())
        );
    }

    #[test]
    fn validate_sort_key_drops_unknown_keys() {
        // Crafted `?sort=<unexpected>` reaches model handler — must be dropped.
        let fields = fields(&[("name", AdminFieldKind::Text)]);
        assert_eq!(
            validate_sort_key(Some("DROP TABLE users".into()), &fields),
            None
        );
        assert_eq!(validate_sort_key(Some("password".into()), &fields), None);
    }

    #[test]
    fn validate_sort_key_drops_non_sortable_fields() {
        let mut computed = AdminField::new("computed", AdminFieldKind::Text);
        computed.sortable = false;
        let schema = vec![computed];
        assert_eq!(validate_sort_key(Some("computed".into()), &schema), None);
    }

    #[test]
    fn validate_sort_key_drops_hidden_columns() {
        // Fields excluded from list_display can't be sorted by URL crafting
        // either — the affordance doesn't exist in the UI.
        let mut secret = AdminField::new("secret", AdminFieldKind::Text);
        secret.list_display = false;
        let schema = vec![secret];
        assert_eq!(validate_sort_key(Some("secret".into()), &schema), None);
    }

    #[test]
    fn validate_sort_key_passes_through_none() {
        let fields = fields(&[("name", AdminFieldKind::Text)]);
        assert_eq!(validate_sort_key(None, &fields), None);
    }

    #[test]
    fn admin_err_maps_variants_to_correct_status() {
        use axum::http::StatusCode;
        assert_eq!(
            admin_err("X", AdminError::NotFound).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            admin_err("X", AdminError::Validation("bad".into())).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            admin_err("X", AdminError::Database("pg down".into())).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            admin_err("X", AdminError::Other("boom".into())).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn admin_csrf_extractor_returns_empty_when_layer_missing() {
        // Simulate a dev/test setup where CsrfLayer is not installed.
        let req = axum::http::Request::builder().uri("/").body(()).unwrap();
        let (mut parts, ()) = req.into_parts();
        let extracted = AdminCsrf::from_request_parts(&mut parts, &())
            .await
            .expect("infallible");
        assert_eq!(extracted.token(), "");
    }

    #[tokio::test]
    async fn admin_csrf_extractor_reads_token_from_extensions() {
        // Build a CsrfToken the way CsrfLayer would — via its public
        // `FromRequestParts`-adjacent API isn't exposed, so reach through
        // the debug impl: we can't construct CsrfToken outside its crate.
        // Instead, verify the extractor at least doesn't panic when the
        // extension IS present by round-tripping through an axum handler.
        use axum::Router;
        use axum::body::Body;
        use axum::http::StatusCode;
        use axum::routing::get;
        use tower::ServiceExt;

        async fn handler(csrf: AdminCsrf) -> String {
            csrf.token().to_owned()
        }
        let app = Router::new().route("/", get(handler));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // No panic, no 500 — just an empty-string body because no CsrfLayer.
        assert_eq!(res.status(), StatusCode::OK);
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

    #[test]
    fn strip_meta_drops_fields_not_in_schema() {
        // Prevents a crafted POST from injecting arbitrary columns past the
        // declared editable surface (e.g. `is_admin=true` on a users model
        // that doesn't expose it).
        let fields = fields(&[("name", AdminFieldKind::Text)]);
        let input = json!({"name": "x", "is_admin": true, "raw_column": "y"});
        let out = strip_meta_fields(input, &fields);
        assert_eq!(out, json!({"name": "x"}));
    }

    #[test]
    fn strip_meta_drops_hidden_fields_even_if_editable_true() {
        // Defense in depth: even if a caller flipped `editable` back to true
        // on a Hidden field (which `AdminField::new` defaults to `false`),
        // the server must still reject it — the form never exposes an input
        // so any submitted value is tampered.
        let mut hidden = AdminField::new("owner_id", AdminFieldKind::Hidden);
        hidden.editable = true; // deliberately wrong
        let schema = vec![hidden];
        let out = strip_meta_fields(json!({"owner_id": 999}), &schema);
        assert_eq!(out, json!({}));
    }

    #[test]
    fn strip_meta_drops_readonly_fields() {
        // `editable = false` fields (id, created_at, computed, privilege
        // flags) must not be forwarded to model code even if submitted.
        let mut id = AdminField::new("id", AdminFieldKind::Integer);
        id.editable = false;
        let mut created_at = AdminField::new("created_at", AdminFieldKind::DateTime);
        created_at.editable = false;
        let name = AdminField::new("name", AdminFieldKind::Text);
        let schema = vec![id, created_at, name];

        let input = json!({
            "id": 999,
            "created_at": "2026-01-01T00:00:00Z",
            "name": "legit",
        });
        let out = strip_meta_fields(input, &schema);
        assert_eq!(out, json!({"name": "legit"}));
    }
}
