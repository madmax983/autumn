//! Maud templates for the admin panel.
//!
//! All templates are server-rendered with HTMX for interactivity.
//! The design mirrors the actuator UI: system-ui font, Tailwind-ish
//! color palette, clean cards with subtle shadows.

use autumn_web::flash::FlashMessage;
use autumn_web::ui::tokens::{FLASH_CSS, TOKENS_CSS};
use autumn_web::{HTMX_CSRF_JS_PATH, HTMX_JS_PATH};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde_json::Value;

use crate::registry::AdminRegistry;
use crate::routes::ADMIN_JS_PATH;
use crate::traits::{AdminField, AdminFieldKind, ListResult, SortDirection, record_id};

// ── CSS ─────────────────────────────────────────────────────────────

/// Admin-specific styles that build on the framework's shared tokens
/// ([`TOKENS_CSS`]) and flash styles ([`FLASH_CSS`]).
const ADMIN_CSS: &str = "
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
        font-family: var(--font-family);
        background: var(--bg);
        color: var(--text);
        line-height: 1.5;
    }
    a { color: var(--primary); text-decoration: none; }
    a:hover { text-decoration: underline; }

    /* Layout */
    .admin-layout { display: flex; min-height: 100vh; }
    .admin-sidebar {
        width: 240px;
        background: var(--surface);
        border-right: 1px solid var(--border);
        padding: 1.5rem 0;
        position: fixed;
        top: 0;
        left: 0;
        bottom: 0;
        overflow-y: auto;
    }
    .admin-main {
        margin-left: 240px;
        flex: 1;
        padding: 2rem;
        min-width: 0;
    }
    .admin-logo {
        font-size: 1.125rem;
        font-weight: 700;
        padding: 0 1.5rem 1rem;
        border-bottom: 1px solid var(--border);
        margin-bottom: 1rem;
        color: var(--text);
    }
    .admin-nav { list-style: none; }
    .admin-nav li a {
        display: block;
        padding: 0.5rem 1.5rem;
        color: var(--text-muted);
        font-size: 0.875rem;
        font-weight: 500;
        border-left: 3px solid transparent;
        transition: all 0.15s;
    }
    .admin-nav li a:hover {
        background: var(--bg);
        color: var(--text);
        text-decoration: none;
    }
    .admin-nav li a.active {
        background: var(--primary-light);
        color: var(--primary);
        border-left-color: var(--primary);
    }
    .admin-nav-section {
        font-size: 0.7rem;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: var(--text-muted);
        padding: 1rem 1.5rem 0.375rem;
        font-weight: 600;
    }

    /* Cards */
    .card {
        background: var(--surface);
        border-radius: var(--radius);
        box-shadow: var(--shadow);
        padding: 1.5rem;
        margin-bottom: 1.5rem;
    }
    .card-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 1rem;
        padding-bottom: 0.75rem;
        border-bottom: 1px solid var(--border);
    }
    .card-title {
        font-size: 1.125rem;
        font-weight: 600;
    }

    /* Buttons */
    .btn {
        display: inline-flex;
        align-items: center;
        gap: 0.375rem;
        padding: 0.5rem 1rem;
        border-radius: 0.375rem;
        font-size: 0.875rem;
        font-weight: 500;
        border: 1px solid var(--border);
        background: var(--surface);
        color: var(--text);
        cursor: pointer;
        transition: all 0.15s;
    }
    .btn:hover { background: var(--bg); text-decoration: none; }
    .btn-primary {
        background: var(--primary);
        color: white;
        border-color: var(--primary);
    }
    .btn-primary:hover { background: var(--primary-hover); }
    .btn-danger {
        background: var(--danger);
        color: white;
        border-color: var(--danger);
    }
    .btn-danger:hover { background: var(--danger-hover); }
    .btn-sm { padding: 0.25rem 0.625rem; font-size: 0.8125rem; }

    /* Tables */
    .table-wrap { overflow-x: auto; }
    table {
        width: 100%;
        border-collapse: collapse;
        font-size: 0.875rem;
    }
    th {
        text-align: left;
        padding: 0.75rem;
        font-weight: 600;
        color: var(--text-muted);
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        border-bottom: 2px solid var(--border);
        white-space: nowrap;
        user-select: none;
    }
    th a { cursor: pointer; }
    th a:hover { color: var(--text); }
    th .sort-icon { font-size: 0.625rem; margin-left: 0.25rem; }
    td {
        padding: 0.75rem;
        border-bottom: 1px solid var(--border);
        max-width: 300px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    tr:hover td { background: var(--bg); }
    .checkbox-cell { width: 40px; text-align: center; }

    /* Forms */
    .form-group { margin-bottom: 1rem; }
    .form-label {
        display: block;
        font-size: 0.875rem;
        font-weight: 500;
        margin-bottom: 0.375rem;
        color: var(--text);
    }
    .form-label .required { color: var(--danger); margin-left: 0.125rem; }
    .form-input {
        width: 100%;
        padding: 0.5rem 0.75rem;
        border: 1px solid var(--border);
        border-radius: 0.375rem;
        font-size: 0.875rem;
        line-height: 1.5;
        background: var(--surface);
        color: var(--text);
        transition: border-color 0.15s;
    }
    .form-input:focus {
        outline: none;
        border-color: var(--primary);
        box-shadow: 0 0 0 3px var(--primary-light);
    }
    textarea.form-input { min-height: 100px; resize: vertical; }
    select.form-input { appearance: auto; }

    /* Search bar */
    .search-bar {
        display: flex;
        gap: 0.75rem;
        margin-bottom: 1rem;
        align-items: center;
    }
    .search-bar input {
        flex: 1;
        padding: 0.5rem 0.75rem;
        border: 1px solid var(--border);
        border-radius: 0.375rem;
        font-size: 0.875rem;
    }
    .search-bar input:focus {
        outline: none;
        border-color: var(--primary);
        box-shadow: 0 0 0 3px var(--primary-light);
    }

    /* Pagination */
    .pagination {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-top: 1rem;
        font-size: 0.875rem;
        color: var(--text-muted);
    }
    .pagination-links {
        display: flex;
        gap: 0.25rem;
    }
    .pagination-links a, .pagination-links span {
        padding: 0.375rem 0.75rem;
        border: 1px solid var(--border);
        border-radius: 0.375rem;
        font-size: 0.8125rem;
        color: var(--text);
    }
    .pagination-links a:hover { background: var(--bg); text-decoration: none; }
    .pagination-links .active {
        background: var(--primary);
        color: white;
        border-color: var(--primary);
    }

    /* Dashboard stats */
    .stats-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
        gap: 1rem;
        margin-bottom: 1.5rem;
    }
    .stat-card {
        background: var(--surface);
        border-radius: var(--radius);
        box-shadow: var(--shadow);
        padding: 1.25rem;
    }
    .stat-label { font-size: 0.8125rem; color: var(--text-muted); font-weight: 500; }
    .stat-value { font-size: 1.75rem; font-weight: 700; margin-top: 0.25rem; }
    .stat-link { font-size: 0.8125rem; margin-top: 0.375rem; }

    /* Breadcrumbs */
    .breadcrumbs {
        font-size: 0.875rem;
        color: var(--text-muted);
        margin-bottom: 1rem;
    }
    .breadcrumbs a { color: var(--text-muted); }
    .breadcrumbs a:hover { color: var(--primary); }
    .breadcrumbs .sep { margin: 0 0.5rem; }

    /* Detail view */
    .detail-grid {
        display: grid;
        grid-template-columns: 160px 1fr;
        gap: 0;
    }
    .detail-label {
        padding: 0.75rem;
        font-weight: 500;
        color: var(--text-muted);
        font-size: 0.875rem;
        border-bottom: 1px solid var(--border);
        background: var(--bg);
    }
    .detail-value {
        padding: 0.75rem;
        font-size: 0.875rem;
        border-bottom: 1px solid var(--border);
        word-break: break-word;
    }

    /* Responsive */
    @media (max-width: 768px) {
        .admin-sidebar { display: none; }
        .admin-main { margin-left: 0; }
        .stats-grid { grid-template-columns: 1fr 1fr; }
    }
    ";

// ── Layout ──────────────────────────────────────────────────────────

/// Render the full admin page layout with sidebar navigation.
#[allow(clippy::too_many_arguments)]
pub fn admin_layout(
    registry: &AdminRegistry,
    active_slug: Option<&str>,
    title: &str,
    prefix: &str,
    actuator_prefix: &str,
    csrf_token: &str,
    messages: &[FlashMessage],
    content: &Markup,
) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                // CSRF token for HTMX requests (hx-delete, hx-post). The
                // companion script at HTMX_CSRF_JS_PATH reads this meta tag
                // and attaches `X-CSRF-Token` to outgoing htmx requests.
                meta name="csrf-token" content=(csrf_token);
                title { (title) " — Autumn Admin" }
                script src=(HTMX_JS_PATH) {}
                script src=(HTMX_CSRF_JS_PATH) {}
                // External so it runs under the default CSP `script-src 'self'`.
                script src={ (prefix) (&**ADMIN_JS_PATH) } {}
                style {
                    (PreEscaped(TOKENS_CSS))
                    (PreEscaped(FLASH_CSS))
                    (PreEscaped(ADMIN_CSS))
                }
            }
            body {
                div class="admin-layout" {
                    // Sidebar
                    nav class="admin-sidebar" {
                        div class="admin-logo" { "🍂 Autumn Admin" }
                        ul class="admin-nav" {
                            li {
                                a href=(prefix) class=[active_slug.is_none().then_some("active")] {
                                    "Dashboard"
                                }
                            }
                            @if registry.model_count() > 0 {
                                li { div class="admin-nav-section" { "Models" } }
                                @for (slug, model) in registry.iter() {
                                    li {
                                        a href={ (prefix) "/" (slug) }
                                          class=[(active_slug == Some(slug)).then_some("active")] {
                                            (model.display_name_plural())
                                        }
                                    }
                                }
                            }
                            li { div class="admin-nav-section" { "System" } }
                            li { a href={ (actuator_prefix) "/ui" } { "Actuator" } }
                        }
                    }
                    // Main content
                    main class="admin-main" {
                        @for msg in messages {
                            div class={ "flash flash-" (msg.level.as_str()) } { (msg.message) }
                        }
                        (content)
                    }
                }
            }
        }
    }
}

// ── Dashboard ───────────────────────────────────────────────────────

/// Render the admin dashboard with model counts.
pub fn dashboard_page(
    registry: &AdminRegistry,
    model_counts: &[(&str, &str, u64)], // (slug, display_name_plural, count)
    messages: &[FlashMessage],
    csrf_token: &str,
    prefix: &str,
    actuator_prefix: &str,
) -> Markup {
    let content = html! {
        h1 style="font-size: 1.5rem; font-weight: 700; margin-bottom: 1.5rem;" {
            "Dashboard"
        }

        div class="stats-grid" {
            @for (slug, name, count) in model_counts {
                div class="stat-card" {
                    div class="stat-label" { (name) }
                    div class="stat-value" { (count) }
                    div class="stat-link" {
                        a href={ (prefix) "/" (slug) } { "View all →" }
                    }
                }
            }
        }

        // Actuator summary (loaded via HTMX)
        div class="card" {
            div class="card-header" {
                span class="card-title" { "System Health" }
                a href={ (actuator_prefix) "/ui" } class="btn btn-sm" { "Full Dashboard →" }
            }
            div hx-get={ (actuator_prefix) "/ui/metrics" } hx-trigger="load, every 5s" {
                "Loading metrics…"
            }
        }
    };
    admin_layout(
        registry,
        None,
        "Dashboard",
        prefix,
        actuator_prefix,
        csrf_token,
        messages,
        &content,
    )
}

// ── Model list view ─────────────────────────────────────────────────

/// Render the list view for a model (table + search + pagination).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn model_list_page(
    registry: &AdminRegistry,
    model_slug: &str,
    model_name_plural: &str,
    fields: &[AdminField],
    result: &ListResult,
    search_query: &str,
    sort_by: Option<&str>,
    sort_dir: SortDirection,
    messages: &[FlashMessage],
    csrf_token: &str,
    prefix: &str,
    actuator_prefix: &str,
) -> Markup {
    // Password fields are documented as write-only — never surface their
    // values (raw or hashed) in the index view, even if the model set
    // `list_display = true`.
    let list_fields: Vec<_> = fields
        .iter()
        .filter(|f| f.list_display && !matches!(f.kind, AdminFieldKind::Password))
        .collect();
    let search_enc = url_encode(search_query);

    let content = html! {
        // Breadcrumbs
        div class="breadcrumbs" {
            a href=(prefix) { "Admin" }
            span class="sep" { "›" }
            span { (model_name_plural) }
        }

        div class="card" {
            div class="card-header" {
                span class="card-title" {
                    (model_name_plural)
                    span style="font-weight: 400; color: var(--text-muted); margin-left: 0.5rem;" {
                        "(" (result.total) ")"
                    }
                }
                a href={ (prefix) "/" (model_slug) "/new" } class="btn btn-primary" {
                    "+ Add " (model_slug.trim_end_matches('s'))
                }
            }

            // Search
            form class="search-bar" method="get" {
                input type="search" name="q" placeholder="Search…"
                    value=(search_query)
                    hx-get={ (prefix) "/" (model_slug) }
                    hx-trigger="input changed delay:300ms"
                    hx-target="closest .card"
                    hx-select=".card > *"
                    hx-push-url="true" {}
            }

            // Table
            div class="table-wrap" {
                table {
                    thead {
                        tr {
                            th class="checkbox-cell" {
                                // Wired up by admin.js via event delegation on #select-all.
                                input type="checkbox" id="select-all";
                            }
                            @for field in &list_fields {
                                @let is_sorted = sort_by == Some(field.name);
                                @let next_dir = if is_sorted { sort_dir.flipped() } else { SortDirection::Asc };
                                th {
                                    @if field.sortable {
                                        a href={ (prefix) "/" (model_slug) "?sort=" (field.name) "&dir=" (next_dir.as_str())
                                            @if !search_enc.is_empty() { "&q=" (search_enc) }
                                        }
                                        style="color: inherit; text-decoration: none;" {
                                            (field.label)
                                            @if is_sorted {
                                                span class="sort-icon" {
                                                    @if matches!(sort_dir, SortDirection::Asc) { "▲" } @else { "▼" }
                                                }
                                            }
                                        }
                                    } @else {
                                        (field.label)
                                    }
                                }
                            }
                            th { "Actions" }
                        }
                    }
                    tbody {
                        @if result.records.is_empty() {
                            tr {
                                td colspan=(list_fields.len() + 2)
                                    style="text-align: center; padding: 2rem; color: var(--text-muted);" {
                                    "No records found."
                                }
                            }
                        }
                        @for record in &result.records {
                            @let row_id = record_id(record);
                            tr {
                                td class="checkbox-cell" {
                                    // Only emit a bulk-action checkbox for rows with a
                                    // routable id — otherwise the form would post id="" or
                                    // the wrong record.
                                    @if let Some(id) = row_id {
                                        input type="checkbox" class="row-check"
                                            name="ids" value=(id);
                                    }
                                }
                                @for field in &list_fields {
                                    td { (render_cell_value(record, field)) }
                                }
                                td {
                                    @if let Some(id) = row_id {
                                        a href={ (prefix) "/" (model_slug) "/" (id) }
                                            class="btn btn-sm" { "View" }
                                        " "
                                        a href={ (prefix) "/" (model_slug) "/" (id) "/edit" }
                                            class="btn btn-sm" { "Edit" }
                                    } @else {
                                        // Surface the issue rather than rendering links to /0.
                                        span style="color: var(--text-muted); font-size: 0.75rem;" {
                                            "no id"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Pagination
            @if result.total_pages() > 1 {
                (render_pagination(result, model_slug, &search_enc, sort_by, sort_dir, prefix))
            }
        }
    };
    admin_layout(
        registry,
        Some(model_slug),
        model_name_plural,
        prefix,
        actuator_prefix,
        csrf_token,
        messages,
        &content,
    )
}

// ── Detail view ─────────────────────────────────────────────────────

/// Render the detail view for a single record.
#[allow(clippy::too_many_arguments)]
pub fn model_detail_page(
    registry: &AdminRegistry,
    model_slug: &str,
    model_name: &str,
    model_name_plural: &str,
    fields: &[AdminField],
    record: &Value,
    record_display: &str,
    // Path-based ID from the handler. Authoritative — edit/delete links
    // must route to the same record the URL addressed, not whatever ID
    // happens to appear in the JSON payload.
    id: i64,
    messages: &[FlashMessage],
    csrf_token: &str,
    prefix: &str,
    actuator_prefix: &str,
) -> Markup {
    let content = html! {
        div class="breadcrumbs" {
            a href=(prefix) { "Admin" }
            span class="sep" { "›" }
            a href={ (prefix) "/" (model_slug) } { (model_name_plural) }
            span class="sep" { "›" }
            span { (record_display) }
        }

        div class="card" {
            div class="card-header" {
                span class="card-title" { (record_display) }
                div {
                    a href={ (prefix) "/" (model_slug) "/" (id) "/edit" }
                        class="btn btn-primary" { "Edit" }
                    " "
                    button class="btn btn-danger"
                        hx-delete={ (prefix) "/" (model_slug) "/" (id) }
                        hx-confirm={ "Are you sure you want to delete this " (model_name) "?" }
                        hx-target="body" {
                        "Delete"
                    }
                }
            }

            div class="detail-grid" {
                @for field in fields {
                    div class="detail-label" { (field.label) }
                    div class="detail-value" {
                        (render_detail_value(record, field))
                    }
                }
            }
        }
    };
    admin_layout(
        registry,
        Some(model_slug),
        record_display,
        prefix,
        actuator_prefix,
        csrf_token,
        messages,
        &content,
    )
}

// ── Create / Edit form ──────────────────────────────────────────────

/// Render the create or edit form for a model.
#[allow(clippy::too_many_arguments)]
pub fn model_form_page(
    registry: &AdminRegistry,
    model_slug: &str,
    model_name: &str,
    model_name_plural: &str,
    fields: &[AdminField],
    record: Option<&Value>,
    // Path-based ID from the handler on edit pages (`None` when rendering
    // the "new" form). Never trust the JSON payload for mutation routing.
    id: Option<i64>,
    messages: &[FlashMessage],
    csrf_token: &str,
    prefix: &str,
    actuator_prefix: &str,
) -> Markup {
    let is_edit = id.is_some();
    let title = if is_edit {
        format!("Edit {model_name}")
    } else {
        format!("New {model_name}")
    };

    let editable_fields: Vec<_> = fields.iter().filter(|f| f.editable).collect();

    let content = html! {
        div class="breadcrumbs" {
            a href=(prefix) { "Admin" }
            span class="sep" { "›" }
            a href={ (prefix) "/" (model_slug) } { (model_name_plural) }
            span class="sep" { "›" }
            span { (title) }
        }

        div class="card" {
            div class="card-header" {
                span class="card-title" { (title) }
            }

            form method="post"
                action={
                    @if let Some(id) = id {
                        (prefix) "/" (model_slug) "/" (id)
                    } @else {
                        (prefix) "/" (model_slug)
                    }
                } {
                input type="hidden" name="_csrf" value=(csrf_token);

                @for field in &editable_fields {
                    div class="form-group" {
                        label class="form-label" for=(field.name) {
                            (field.label)
                            @if field.required {
                                span class="required" { "*" }
                            }
                        }
                        (render_form_widget(field, record))
                    }
                }

                div style="display: flex; gap: 0.75rem; margin-top: 1.5rem;" {
                    button type="submit" class="btn btn-primary" {
                        @if is_edit { "Save Changes" } @else { "Create" }
                    }
                    a href={ (prefix) "/" (model_slug) } class="btn" {
                        "Cancel"
                    }
                }
            }
        }
    };
    admin_layout(
        registry,
        Some(model_slug),
        &title,
        prefix,
        actuator_prefix,
        csrf_token,
        messages,
        &content,
    )
}

// ── Rendering helpers ───────────────────────────────────────────────

/// Render a cell value in the list table.
fn render_cell_value(record: &Value, field: &AdminField) -> Markup {
    // Defense in depth: the list-view field filter already excludes
    // `AdminFieldKind::Password`, but mask here too so we can never leak
    // a hash if a caller slips one through.
    if matches!(field.kind, AdminFieldKind::Password) {
        return html! { "••••••••" };
    }
    let val = record.get(field.name);
    match val {
        None | Some(Value::Null) => html! {
            span style="color: var(--text-muted);" { "—" }
        },
        Some(Value::Bool(b)) => html! {
            @if *b {
                span style="color: var(--success);" { "✓" }
            } @else {
                span style="color: var(--text-muted);" { "✗" }
            }
        },
        Some(Value::String(s)) => html! { (truncate_display(s, 80)) },
        Some(v) => html! { (v) },
    }
}

/// Percent-encode a query-component value.
///
/// Conservative: escapes everything that isn't an unreserved URL character
/// (RFC 3986 `unreserved`: `A-Z a-z 0-9 - . _ ~`). Safe for both path and
/// query contexts.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char);
            }
            other => {
                use std::fmt::Write;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// UTF-8-safe truncation by character count. Appends `…` if truncated.
fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Normalize a stored date string into `YYYY-MM-DD`, the only format the
/// HTML `<input type="date">` control accepts. Leaves the input untouched
/// if it can't be parsed — the user sees whatever the backend sent rather
/// than a silently-empty field.
fn normalize_date_input(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    // Fast path: already in the right shape.
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return s.to_owned();
    }
    // Fall back to full RFC 3339 (which includes the `T` + time).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.format("%Y-%m-%d").to_string();
    }
    // Finally try a naive datetime.
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return ndt.format("%Y-%m-%d").to_string();
    }
    s.to_owned()
}

/// Normalize a stored datetime string into `YYYY-MM-DDTHH:MM`, the only
/// format the HTML `<input type="datetime-local">` control accepts.
/// Browsers silently reject RFC 3339 with `Z`/offset; this maps common
/// backend representations onto the local-time shape the input expects.
///
/// Timezone-aware inputs are converted to UTC (same instant, different
/// representation). If parsing fails, the original string is returned
/// unchanged — better to show the server's value than silently blank the
/// field on edit.
fn normalize_datetime_local_input(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    // Already local-shaped.
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").is_ok() {
        return s.to_owned();
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return ndt.format("%Y-%m-%dT%H:%M").to_string();
    }
    // RFC 3339 with timezone — serialize as UTC, drop the offset.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.naive_utc().format("%Y-%m-%dT%H:%M").to_string();
    }
    s.to_owned()
}

/// Render a field value in the detail view.
fn render_detail_value(record: &Value, field: &AdminField) -> Markup {
    let val = record.get(field.name);
    match val {
        None | Some(Value::Null) => html! {
            span style="color: var(--text-muted);" { "—" }
        },
        Some(Value::Bool(b)) => html! {
            @if *b { "Yes" } @else { "No" }
        },
        Some(Value::String(s)) => {
            if matches!(field.kind, AdminFieldKind::Password) {
                html! { "••••••••" }
            } else if matches!(field.kind, AdminFieldKind::TextArea | AdminFieldKind::Json) {
                html! {
                    pre style="white-space: pre-wrap; font-size: 0.8125rem; background: var(--bg); padding: 0.75rem; border-radius: 0.375rem;" {
                        (s)
                    }
                }
            } else {
                html! { (s) }
            }
        }
        // Objects and arrays pretty-printed inside <pre>; plain text so
        // Maud HTML-escapes attacker-controlled content. PreEscaped here
        // would be a stored-XSS sink.
        Some(v) => html! {
            pre style="white-space: pre-wrap; font-size: 0.8125rem; background: var(--bg); padding: 0.75rem; border-radius: 0.375rem;" {
                (serde_json::to_string_pretty(v).unwrap_or_default())
            }
        },
    }
}

/// Render a form widget for a field.
fn render_form_widget(field: &AdminField, record: Option<&Value>) -> Markup {
    let current_value = record
        .and_then(|r| r.get(field.name))
        .cloned()
        .unwrap_or(Value::Null);
    let str_val = match &current_value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    };

    match &field.kind {
        AdminFieldKind::Text => html! {
            input type="text" class="form-input" name=(field.name) id=(field.name)
                value=(str_val)
                required[field.required];
        },
        AdminFieldKind::TextArea => html! {
            textarea class="form-input" name=(field.name) id=(field.name)
                required[field.required] {
                (str_val)
            }
        },
        AdminFieldKind::Integer => html! {
            input type="number" class="form-input" name=(field.name) id=(field.name)
                value=(str_val) step="1"
                required[field.required];
        },
        AdminFieldKind::Float => html! {
            input type="number" class="form-input" name=(field.name) id=(field.name)
                value=(str_val) step="any"
                required[field.required];
        },
        AdminFieldKind::Boolean => {
            let checked = matches!(current_value, Value::Bool(true));
            html! {
                input type="hidden" name=(field.name) value="false";
                input type="checkbox" name=(field.name) id=(field.name)
                    value="true" checked[checked]
                    style="width: auto;";
            }
        }
        AdminFieldKind::Date => {
            let v = normalize_date_input(&str_val);
            html! {
                input type="date" class="form-input" name=(field.name) id=(field.name)
                    value=(v)
                    required[field.required];
            }
        }
        AdminFieldKind::DateTime => {
            let v = normalize_datetime_local_input(&str_val);
            html! {
                input type="datetime-local" class="form-input" name=(field.name) id=(field.name)
                    value=(v)
                    required[field.required];
            }
        }
        AdminFieldKind::Select(options) => html! {
            select class="form-input" name=(field.name) id=(field.name)
                required[field.required] {
                option value="" { "— Select —" }
                @for opt in options {
                    option value=(opt.value)
                        selected[str_val == opt.value] {
                        (opt.label)
                    }
                }
            }
        },
        AdminFieldKind::Hidden => html! {
            input type="hidden" name=(field.name) value=(str_val);
        },
        AdminFieldKind::Password => html! {
            input type="password" class="form-input" name=(field.name) id=(field.name)
                placeholder="Leave blank to keep current"
                autocomplete="new-password";
        },
        AdminFieldKind::Json => html! {
            textarea class="form-input" name=(field.name) id=(field.name)
                style="font-family: monospace; min-height: 150px;"
                required[field.required] {
                (str_val)
            }
        },
    }
}

/// Render pagination controls.
///
/// `search_enc` is expected to be already URL-encoded; callers pass the raw
/// form for rendering and the encoded form for link building.
fn render_pagination(
    result: &ListResult,
    model_slug: &str,
    search_enc: &str,
    sort_by: Option<&str>,
    sort_dir: SortDirection,
    prefix: &str,
) -> Markup {
    let total_pages = result.total_pages();
    let current = result.page.max(1);

    // The fixed portion of the query string — built once, reused per link.
    let suffix = {
        let mut s = String::new();
        if !search_enc.is_empty() {
            s.push_str("&q=");
            s.push_str(search_enc);
        }
        if let Some(sort) = sort_by {
            s.push_str("&sort=");
            s.push_str(&url_encode(sort));
            s.push_str("&dir=");
            s.push_str(sort_dir.as_str());
        }
        s
    };
    let base_qs = |page: u64| -> String { format!("{prefix}/{model_slug}?page={page}{suffix}") };

    let start = if result.total == 0 {
        0
    } else {
        result.per_page.saturating_mul(current - 1) + 1
    };
    let end = start
        .saturating_add(result.per_page)
        .saturating_sub(1)
        .min(result.total);

    html! {
        div class="pagination" {
            span {
                "Showing " (start) "–" (end) " of " (result.total)
            }
            div class="pagination-links" {
                @if current > 1 {
                    a href=(base_qs(current - 1)) { "← Prev" }
                }
                @for page in pagination_range(current, total_pages) {
                    @if page == 0 {
                        span style="border: none; color: var(--text-muted);" { "…" }
                    } @else if page == current {
                        span class="active" { (page) }
                    } @else {
                        a href=(base_qs(page)) { (page) }
                    }
                }
                @if current < total_pages {
                    a href=(base_qs(current + 1)) { "Next →" }
                }
            }
        }
    }
}

/// Generate a pagination range with ellipsis (0 = ellipsis marker).
fn pagination_range(current: u64, total: u64) -> Vec<u64> {
    if total <= 7 {
        return (1..=total).collect();
    }
    let mut pages = Vec::new();
    pages.push(1);
    if current > 3 {
        pages.push(0); // ellipsis
    }
    let start = current.saturating_sub(1).max(2);
    let end = (current + 1).min(total - 1);
    for p in start..=end {
        pages.push(p);
    }
    if current < total - 2 {
        pages.push(0); // ellipsis
    }
    if *pages.last().unwrap_or(&0) != total {
        pages.push(total);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_range_small() {
        assert_eq!(pagination_range(1, 5), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pagination_range_middle() {
        let result = pagination_range(5, 10);
        assert!(result.contains(&1));
        assert!(result.contains(&5));
        assert!(result.contains(&10));
        assert!(result.contains(&0)); // ellipsis
    }

    #[test]
    fn pagination_range_start() {
        let result = pagination_range(1, 10);
        assert_eq!(result[0], 1);
        assert_eq!(result[1], 2);
    }

    #[test]
    fn pagination_range_end() {
        let result = pagination_range(10, 10);
        assert_eq!(*result.last().unwrap(), 10);
    }

    #[test]
    fn truncate_display_ascii() {
        assert_eq!(truncate_display("hello", 10), "hello");
        assert_eq!(truncate_display("hello world!", 6), "hello…");
    }

    #[test]
    fn truncate_display_utf8_boundary_safe() {
        // 4 multi-byte chars, each 3 bytes. Byte slicing at 7 would panic.
        let s = "日本語日";
        assert_eq!(truncate_display(s, 3), "日本…");
        // Under the limit by char count, no truncation.
        assert_eq!(truncate_display(s, 10), s);
    }

    #[test]
    fn url_encode_handles_reserved_chars() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(url_encode("safe-._~"), "safe-._~");
    }

    #[test]
    fn url_encode_handles_utf8() {
        // "é" is 0xC3 0xA9 in UTF-8.
        assert_eq!(url_encode("é"), "%C3%A9");
    }

    #[test]
    fn normalize_datetime_local_accepts_expected_shape() {
        assert_eq!(
            normalize_datetime_local_input("2026-04-24T12:34"),
            "2026-04-24T12:34"
        );
    }

    #[test]
    fn normalize_datetime_local_strips_seconds() {
        assert_eq!(
            normalize_datetime_local_input("2026-04-24T12:34:56"),
            "2026-04-24T12:34"
        );
    }

    #[test]
    fn normalize_datetime_local_strips_rfc3339_zulu() {
        // The browser refuses the `Z` suffix; we emit UTC without offset.
        assert_eq!(
            normalize_datetime_local_input("2026-04-24T12:34:56Z"),
            "2026-04-24T12:34"
        );
    }

    #[test]
    fn normalize_datetime_local_strips_rfc3339_offset() {
        // +05:30 offset → converted to UTC (07:04), then rendered local-shaped.
        assert_eq!(
            normalize_datetime_local_input("2026-04-24T12:34:56+05:30"),
            "2026-04-24T07:04"
        );
    }

    #[test]
    fn normalize_datetime_local_empty_stays_empty() {
        assert_eq!(normalize_datetime_local_input(""), "");
    }

    #[test]
    fn normalize_datetime_local_leaves_garbage_untouched() {
        // Better to show the raw value than silently blank the field.
        assert_eq!(normalize_datetime_local_input("not-a-date"), "not-a-date");
    }

    #[test]
    fn normalize_date_accepts_expected_shape() {
        assert_eq!(normalize_date_input("2026-04-24"), "2026-04-24");
    }

    #[test]
    fn normalize_date_extracts_from_rfc3339() {
        assert_eq!(normalize_date_input("2026-04-24T12:34:56Z"), "2026-04-24");
    }

    // ── End-to-end render checks (CSRF / XSS / actuator prefix wiring) ──

    fn dummy_registry() -> AdminRegistry {
        AdminRegistry::new()
    }

    #[test]
    fn dashboard_emits_csrf_meta_and_script() {
        let r = dummy_registry();
        let html = dashboard_page(&r, &[], &[], "tok-123", "/admin", "/ops").into_string();
        assert!(
            html.contains(r#"<meta name="csrf-token" content="tok-123""#),
            "CSRF meta tag missing: {html}"
        );
        assert!(
            html.contains("/static/js/autumn-htmx-csrf.js"),
            "HTMX CSRF helper script not loaded: {html}"
        );
    }

    #[test]
    fn dashboard_uses_configured_actuator_prefix() {
        let r = dummy_registry();
        let html = dashboard_page(&r, &[], &[], "tok", "/admin", "/ops").into_string();
        assert!(
            html.contains(r#"href="/ops/ui""#),
            "sidebar link wrong: {html}"
        );
        assert!(
            html.contains(r#"hx-get="/ops/ui/metrics""#),
            "metrics polling URL wrong: {html}"
        );
        assert!(
            !html.contains("/actuator/"),
            "must not hardcode /actuator when prefix is /ops: {html}"
        );
    }

    #[test]
    fn form_page_renders_hidden_csrf_input() {
        let r = dummy_registry();
        let fields = vec![AdminField::new("name", AdminFieldKind::Text)];
        let html = model_form_page(
            &r,
            "widgets",
            "Widget",
            "Widgets",
            &fields,
            None,
            None,
            &[],
            "tok-xyz",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            html.contains(r#"<input type="hidden" name="_csrf" value="tok-xyz""#),
            "_csrf hidden field missing: {html}"
        );
    }

    #[test]
    fn form_page_normalizes_datetime_for_browser_input() {
        let r = dummy_registry();
        let fields = vec![AdminField::new("created_at", AdminFieldKind::DateTime)];
        // RFC 3339 with `Z` — would render as empty without normalization.
        let record = serde_json::json!({"id": 1, "created_at": "2026-04-24T12:34:56Z"});
        let html = model_form_page(
            &r,
            "widgets",
            "Widget",
            "Widgets",
            &fields,
            Some(&record),
            Some(1),
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            html.contains(r#"value="2026-04-24T12:34""#),
            "datetime-local input should carry browser-friendly value: {html}"
        );
        assert!(
            !html.contains(r#"value="2026-04-24T12:34:56Z""#),
            "raw RFC3339 must not reach datetime-local input: {html}"
        );
    }

    #[test]
    fn form_page_action_uses_path_id_not_payload_id() {
        // Regression: mutation target must come from the URL path, not the
        // record payload. Payload says id=99, path says 42 — form posts to 42.
        let r = dummy_registry();
        let fields = vec![AdminField::new("name", AdminFieldKind::Text)];
        let record = serde_json::json!({"id": 99, "name": "x"});
        let html = model_form_page(
            &r,
            "widgets",
            "Widget",
            "Widgets",
            &fields,
            Some(&record),
            Some(42),
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            html.contains(r#"action="/admin/widgets/42""#),
            "form action should use path-based id 42, not payload id 99: {html}"
        );
        assert!(
            !html.contains(r#"action="/admin/widgets/99""#),
            "payload-derived id must not appear in form action: {html}"
        );
    }

    #[test]
    fn detail_page_edit_delete_links_use_path_id() {
        let r = dummy_registry();
        let fields = vec![AdminField::new("name", AdminFieldKind::Text)];
        let record = serde_json::json!({"id": 99, "name": "x"});
        let html = model_detail_page(
            &r,
            "widgets",
            "Widget",
            "Widgets",
            &fields,
            &record,
            "#42",
            42,
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            html.contains(r#"href="/admin/widgets/42/edit""#),
            "Edit link must use path id 42: {html}"
        );
        assert!(
            html.contains(r#"hx-delete="/admin/widgets/42""#),
            "Delete must target path id 42: {html}"
        );
        assert!(
            !html.contains("widgets/99"),
            "payload id 99 must not route mutations: {html}"
        );
    }

    #[test]
    fn detail_view_escapes_malicious_json() {
        let r = dummy_registry();
        let fields = vec![AdminField::new("meta", AdminFieldKind::Json)];
        // Pretty-printed JSON of a nested object contains attacker-controlled
        // angle brackets. Pre-fix this rendered as live HTML.
        let record = serde_json::json!({
            "id": 1,
            "meta": {"xss": "<script>alert(1)</script>"},
        });
        let html = model_detail_page(
            &r,
            "widgets",
            "Widget",
            "Widgets",
            &fields,
            &record,
            "#1",
            1,
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "raw <script> must be escaped: {html}"
        );
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "escaped form expected: {html}"
        );
    }

    #[test]
    fn layout_loads_external_admin_js_not_inline() {
        // The layout must NOT ship an inline <script>{js}</script> block —
        // that would be blocked by the default CSP (`script-src 'self'`).
        // Instead it must load the plugin-owned asset at a fingerprinted
        // `/{prefix}/static/admin.<hash>.js` URL.
        let r = dummy_registry();
        let html = dashboard_page(&r, &[], &[], "t", "/admin", "/actuator").into_string();
        let expected = format!(r#"src="/admin{}""#, &**ADMIN_JS_PATH);
        assert!(
            html.contains(&expected),
            "admin.js must be referenced as an external script at {expected}: {html}"
        );
        // The URL must be content-fingerprinted so immutable caching is safe.
        assert!(
            html.contains("/admin/static/admin.") && html.contains(".js\""),
            "admin.js URL should be fingerprinted (admin.<hash>.js): {html}"
        );
        assert!(
            !html.contains(r#"src="/admin/static/admin.js""#),
            "unfingerprinted URL would invalidate immutable caching: {html}"
        );
        // No inline onclick on the select-all checkbox either — it's
        // wired via event delegation in admin.js now.
        assert!(
            !html.contains("onclick=\""),
            "no inline event handlers allowed under default CSP: {html}"
        );
    }

    #[test]
    fn list_page_hides_password_fields_even_if_list_display_true() {
        use crate::traits::ListResult;
        let r = dummy_registry();
        // A model that (incorrectly) marks a password field list_display=true.
        // The admin plugin must drop it from the index table anyway.
        let fields = vec![
            AdminField::new("name", AdminFieldKind::Text),
            AdminField::new("password_hash", AdminFieldKind::Password),
        ];
        let result = ListResult {
            records: vec![serde_json::json!({
                "id": 1,
                "name": "alice",
                "password_hash": "$argon2id$leaked",
            })],
            total: 1,
            page: 1,
            per_page: 25,
        };
        let html = model_list_page(
            &r,
            "users",
            "Users",
            &fields,
            &result,
            "",
            None,
            SortDirection::Asc,
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        assert!(
            !html.contains("$argon2id$leaked"),
            "raw password hash must not appear in list view: {html}"
        );
        assert!(
            !html.contains("password_hash"),
            "password column must not have a header in list view: {html}"
        );
    }

    #[test]
    fn list_page_handles_records_without_numeric_id() {
        // A model that returns a row whose `id` is missing (or non-numeric)
        // must not render `/admin/widgets/0` action links — those would
        // route mutations to the wrong row. Show "no id" instead.
        use crate::traits::ListResult;
        let r = dummy_registry();
        let fields = vec![AdminField::new("name", AdminFieldKind::Text)];
        let result = ListResult {
            records: vec![
                serde_json::json!({"id": 7, "name": "with id"}),
                serde_json::json!({"name": "no id"}),
            ],
            total: 2,
            page: 1,
            per_page: 25,
        };
        let html = model_list_page(
            &r,
            "widgets",
            "Widgets",
            &fields,
            &result,
            "",
            None,
            SortDirection::Asc,
            &[],
            "t",
            "/admin",
            "/actuator",
        )
        .into_string();
        // Row with id renders working links and a checkbox.
        assert!(
            html.contains(r#"href="/admin/widgets/7""#),
            "row with id should have working View link: {html}"
        );
        // Row without id shows the placeholder, never `/0` links.
        assert!(
            html.contains(r#"<span style="color: var(--text-muted); font-size: 0.75rem;">no id"#)
                || html.contains("no id</span>"),
            "row without id should show 'no id' placeholder: {html}"
        );
        assert!(
            !html.contains("/admin/widgets/0"),
            "must not generate /0 links for rows missing id: {html}"
        );
    }

    #[test]
    fn list_page_omits_sort_link_for_unsortable_fields() {
        use crate::traits::ListResult;
        let r = dummy_registry();
        // One sortable, one non-sortable.
        let mut computed = AdminField::new("computed", AdminFieldKind::Text).label("Computed");
        computed.sortable = false;
        let fields = vec![AdminField::new("name", AdminFieldKind::Text), computed];
        let result = ListResult {
            records: vec![],
            total: 0,
            page: 1,
            per_page: 25,
        };
        let html = model_list_page(
            &r,
            "widgets",
            "Widgets",
            &fields,
            &result,
            "",
            None,
            SortDirection::Asc,
            &[],
            "tok",
            "/admin",
            "/actuator",
        )
        .into_string();
        // Sortable field gets a sort link.
        assert!(
            html.contains(r#"href="/admin/widgets?sort=name"#),
            "sortable field should have a sort link: {html}"
        );
        // Non-sortable field must NOT get a sort link.
        assert!(
            !html.contains("sort=computed"),
            "non-sortable field must not emit a sort link: {html}"
        );
        // But its label is still rendered.
        assert!(
            html.contains("Computed"),
            "label should still render: {html}"
        );
    }

    #[test]
    fn admin_js_does_not_contain_inline_event_handlers() {
        // Sanity-check the shipped JS: has the two behaviours we expect.
        let js = include_str!("admin.js");
        assert!(
            js.contains("select-all"),
            "admin.js should wire the select-all checkbox"
        );
        assert!(
            js.contains("removeAttribute(\"name\")"),
            "admin.js should strip blank password input names"
        );
    }
}
