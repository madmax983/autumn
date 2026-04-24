//! Core traits that models implement to participate in the admin panel.

use std::future::Future;
use std::pin::Pin;

use serde::Deserialize;
use serde_json::Value;

// ── Field metadata ──────────────────────────────────────────────────

/// The kind of a model field, used to select the appropriate form widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminFieldKind {
    /// Single-line text input.
    Text,
    /// Multi-line textarea.
    TextArea,
    /// Integer input.
    Integer,
    /// Floating-point input.
    Float,
    /// Boolean checkbox.
    Boolean,
    /// Date picker (no time).
    Date,
    /// Date + time picker.
    DateTime,
    /// Select dropdown with fixed choices.
    Select(Vec<SelectOption>),
    /// Hidden field (shown in detail, not editable).
    Hidden,
    /// Password field (write-only, never displayed).
    Password,
    /// JSON editor.
    Json,
}

/// A single option in a [`AdminFieldKind::Select`] dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// Metadata for a single model field.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // orthogonal flags on a plain config record
pub struct AdminField {
    /// Column name in the database / struct field name.
    pub name: &'static str,
    /// Human-readable label for the UI.
    pub label: String,
    /// Widget type.
    pub kind: AdminFieldKind,
    /// Whether this field appears in the list view table.
    pub list_display: bool,
    /// Whether this field is searchable (included in text search).
    pub searchable: bool,
    /// Whether this field can be used as a filter.
    pub filterable: bool,
    /// Whether this field is required on create/edit forms.
    pub required: bool,
    /// Whether this field is editable (false for IDs, timestamps, etc.).
    pub editable: bool,
    /// Sort priority in list view (None = not sortable).
    pub sortable: bool,
}

impl AdminField {
    /// Create a new field with sensible defaults.
    ///
    /// By default: displayed in list, not searchable, not filterable,
    /// required, and sortable. Editable defaults to `true` except for
    /// [`AdminFieldKind::Hidden`], which is read-only by contract
    /// (and is therefore excluded from `strip_meta_fields` acceptance
    /// even if a caller later flips `editable` back to `true`).
    #[must_use]
    pub fn new(name: &'static str, kind: AdminFieldKind) -> Self {
        let editable = !matches!(kind, AdminFieldKind::Hidden);
        Self {
            name,
            label: humanize_field_name(name),
            kind,
            list_display: true,
            searchable: false,
            filterable: false,
            required: true,
            editable,
            sortable: true,
        }
    }

    /// Set the human-readable label.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Mark this field as searchable.
    #[must_use]
    pub const fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }

    /// Mark this field as filterable.
    #[must_use]
    pub const fn filterable(mut self) -> Self {
        self.filterable = true;
        self
    }

    /// Mark this field as optional.
    #[must_use]
    pub const fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Mark this field as read-only.
    #[must_use]
    pub const fn readonly(mut self) -> Self {
        self.editable = false;
        self
    }

    /// Hide this field from the list view.
    #[must_use]
    pub const fn hide_from_list(mut self) -> Self {
        self.list_display = false;
        self
    }
}

// ── Bulk actions ────────────────────────────────────────────────────

/// A named bulk action that can be performed on selected records.
pub struct AdminAction {
    /// Machine name (used in form values).
    pub name: &'static str,
    /// Human-readable label for the button.
    pub label: String,
    /// CSS class for styling (e.g., "danger" for destructive actions).
    pub style: ActionStyle,
    /// Whether a confirmation dialog is shown before executing.
    pub confirm: bool,
}

/// Visual style for an admin action button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStyle {
    /// Default/neutral style.
    Default,
    /// Primary/positive action.
    Primary,
    /// Destructive/dangerous action (red).
    Danger,
}

// ── The core trait ──────────────────────────────────────────────────

/// Type alias for the boxed future returned by async `AdminModel` methods.
pub type AdminFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AdminError>> + Send + 'a>>;

/// Error type for admin operations.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("Record not found")]
    NotFound,

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("{0}")]
    Other(String),
}

/// The core trait that enables a model to be managed in the admin panel.
///
/// Implementors provide field metadata, CRUD operations, and display
/// configuration. The admin plugin uses this trait to generate all views
/// dynamically at runtime.
///
/// # Design notes
///
/// All data flows through `serde_json::Value` to keep the trait object-safe.
/// The admin panel doesn't need to know concrete types — it renders fields
/// based on [`AdminField`] metadata and passes values as JSON.
pub trait AdminModel: Send + Sync + 'static {
    /// URL-safe slug for this model (e.g., "projects", "tickets").
    /// Used in admin URLs: `/admin/projects/`, `/admin/projects/42/`.
    fn slug(&self) -> &'static str;

    /// Human-readable singular name (e.g., "Project").
    fn display_name(&self) -> &'static str;

    /// Human-readable plural name (e.g., "Projects").
    fn display_name_plural(&self) -> &'static str;

    /// Field metadata for this model.
    fn fields(&self) -> Vec<AdminField>;

    /// Available bulk actions (default: just "Delete selected").
    fn actions(&self) -> Vec<AdminAction> {
        vec![AdminAction {
            name: "delete",
            label: "Delete selected".to_owned(),
            style: ActionStyle::Danger,
            confirm: true,
        }]
    }

    // ── CRUD operations ─────────────────────────────────────────

    /// List records with pagination, search, sort, and filters.
    fn list(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        params: ListParams,
    ) -> AdminFuture<'_, ListResult>;

    /// Get a single record by ID.
    fn get(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
    ) -> AdminFuture<'_, Option<Value>>;

    /// Create a new record from form data.
    fn create(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        data: Value,
    ) -> AdminFuture<'_, Value>;

    /// Update an existing record.
    fn update(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
        data: Value,
    ) -> AdminFuture<'_, Value>;

    /// Delete a record by ID.
    fn delete(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
    ) -> AdminFuture<'_, ()>;

    /// Execute a bulk action on the given IDs.
    fn execute_action(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        action: &str,
        ids: Vec<i64>,
    ) -> AdminFuture<'_, u64> {
        let _ = (pool, action, ids);
        Box::pin(async { Ok(0) })
    }

    /// Return a display string for a record (used in breadcrumbs, titles).
    ///
    /// Defaults to `"ModelName #id"`.
    fn record_display(&self, record: &Value) -> String {
        format!("{} #{}", self.display_name(), record_id(record))
    }

    /// Records per page in the list view. Override to taste.
    fn per_page(&self) -> u64 {
        25
    }

    /// Count records matching a list query (defaults to `list(..., per_page: 0).total`).
    ///
    /// Override if the backend can count without materializing records.
    fn count(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    ) -> AdminFuture<'_, u64> {
        let params = ListParams {
            page: 1,
            per_page: 0,
            ..Default::default()
        };
        let fut = self.list(pool, params);
        Box::pin(async move { fut.await.map(|r| r.total) })
    }
}

/// Extract the `"id"` field of a record as `i64`, defaulting to `0`.
#[must_use]
pub fn record_id(record: &Value) -> i64 {
    record.get("id").and_then(Value::as_i64).unwrap_or(0)
}

// ── Query parameters ────────────────────────────────────────────────

/// Parameters for a list query.
#[derive(Debug, Clone, Default)]
pub struct ListParams {
    /// Page number (1-indexed).
    pub page: u64,
    /// Records per page.
    pub per_page: u64,
    /// Full-text search query.
    pub search: Option<String>,
    /// Column to sort by.
    pub sort_by: Option<String>,
    /// Sort direction.
    pub sort_dir: SortDirection,
    /// Active filters (`field_name` → value).
    pub filters: Vec<(String, String)>,
}

/// Sort direction for list queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

impl SortDirection {
    /// URL-friendly representation (`"asc"` / `"desc"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    /// The opposite direction (used to flip sort on re-click).
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

/// Result of a list query, containing records and pagination metadata.
#[derive(Debug, Clone)]
pub struct ListResult {
    /// The records for the current page (as JSON objects).
    pub records: Vec<Value>,
    /// Total number of records matching the query (for pagination).
    pub total: u64,
    /// Current page number.
    pub page: u64,
    /// Records per page.
    pub per_page: u64,
}

impl ListResult {
    /// Total number of pages.
    #[must_use]
    pub const fn total_pages(&self) -> u64 {
        if self.per_page == 0 {
            return 0;
        }
        self.total.div_ceil(self.per_page)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Convert a `snake_case` field name to a human-readable label.
///
/// `"created_at"` → `"Created At"`, `"user_id"` → `"User Id"`.
fn humanize_field_name(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                let mut s = c.to_uppercase().to_string();
                s.extend(chars);
                s
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_converts_snake_case() {
        assert_eq!(humanize_field_name("created_at"), "Created At");
        assert_eq!(humanize_field_name("user_id"), "User Id");
        assert_eq!(humanize_field_name("name"), "Name");
        assert_eq!(humanize_field_name(""), "");
    }

    #[test]
    fn list_result_total_pages() {
        let result = ListResult {
            records: vec![],
            total: 25,
            page: 1,
            per_page: 10,
        };
        assert_eq!(result.total_pages(), 3);
    }

    #[test]
    fn list_result_total_pages_exact() {
        let result = ListResult {
            records: vec![],
            total: 20,
            page: 1,
            per_page: 10,
        };
        assert_eq!(result.total_pages(), 2);
    }

    #[test]
    fn list_result_total_pages_zero_per_page() {
        let result = ListResult {
            records: vec![],
            total: 20,
            page: 1,
            per_page: 0,
        };
        assert_eq!(result.total_pages(), 0);
    }

    #[test]
    fn admin_field_builder() {
        let field = AdminField::new("email", AdminFieldKind::Text)
            .label("Email Address")
            .searchable()
            .filterable()
            .optional();

        assert_eq!(field.name, "email");
        assert_eq!(field.label, "Email Address");
        assert!(field.searchable);
        assert!(field.filterable);
        assert!(!field.required);
        assert!(field.editable);
    }

    #[test]
    fn hidden_fields_default_to_not_editable() {
        // AdminFieldKind::Hidden is documented as "not editable". Ensure the
        // default matches the contract so admins who skip `.readonly()` still
        // get safe behaviour.
        let hidden = AdminField::new("owner_id", AdminFieldKind::Hidden);
        assert!(
            !hidden.editable,
            "Hidden fields must default to editable=false"
        );

        // Other kinds remain editable by default.
        let text = AdminField::new("name", AdminFieldKind::Text);
        assert!(text.editable);
    }
}
