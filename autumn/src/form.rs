//! Changeset-style form helpers with validation and Maud rendering.
//!
//! # Overview
//!
//! [`Changeset<T>`] captures submitted form values together with per-field
//! validation errors, enabling the create/edit/validate-failure round-trip
//! in a single route handler — no manual flash-carrying, no conditional
//! error-threading.
//!
//! [`ChangesetForm<T>`] is the axum extractor that decodes the request body
//! (URL-encoded **or** multipart), runs validation, captures the CSRF token,
//! and hands the handler a ready-to-use changeset — CSRF is emitted
//! automatically when you call [`ChangesetForm::form_tag`].
//!
//! # Framework comparison
//!
//! | Framework | Changeset type | Rendering helper |
//! |-----------|---------------|-----------------|
//! | Phoenix (Elixir) | `Ecto.Changeset` | `<.input field={@form[:name]} />` |
//! | Rails (Ruby) | `errors[:field]` | `f.text_field :name` |
//! | Django (Python) | `forms.Form` | `{{ form.name.errors }}` |
//! | Autumn (Rust) | `Changeset<T>` | `form.text_input("name", "Name")` |
//!
//! # Happy-path + validation-failure in ≤ 40 `LoC`
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::form::{ChangesetForm, Changeset, submit_button};
//! use serde::{Deserialize, Serialize};
//! use validator::Validate;
//! use axum::{http::StatusCode, response::IntoResponse};
//!
//! #[derive(Deserialize, Serialize, Validate)]
//! struct GreetForm {
//!     #[validate(length(min = 3, message = "Name must be at least 3 characters"))]
//!     name: String,
//!     #[validate(email(message = "Must be a valid email address"))]
//!     email: String,
//! }
//!
//! fn greet_form_partial(form: &ChangesetForm<GreetForm>, action: &str) -> Markup {
//!     form.form_tag(action, "post", html! {
//!         (form.text_input("name", "Full name"))
//!         (form.text_input("email", "Email"))
//!         (form.submit_button("Submit"))
//!     })
//! }
//!
//! #[get("/greet/new")]
//! async fn new_greet(csrf: CsrfToken) -> Markup {
//!     let blank = ChangesetForm::blank(GreetForm { name: String::new(), email: String::new() },
//!                                     csrf.token());
//!     greet_form_partial(&blank, "/greet")
//! }
//!
//! #[post("/greet")]
//! async fn create_greet(form: ChangesetForm<GreetForm>) -> impl IntoResponse {
//!     match form.into_valid() {
//!         Ok(f) => html! { p { "Hello, " (f.name) "!" } }.into_response(),
//!         Err(form) => (StatusCode::UNPROCESSABLE_ENTITY,
//!                       greet_form_partial(&form, "/greet")).into_response(),
//!     }
//! }
//! ```
//!
//! # CSRF
//!
//! The CSRF token is captured automatically by the [`ChangesetForm`] extractor
//! from the request extensions set by [`crate::security::CsrfLayer`].
//! Calling [`ChangesetForm::form_tag`] emits the hidden `_csrf` input with no
//! additional developer action in POST handlers.
//!
//! For GET handlers (new/edit forms), construct the form context via
//! [`ChangesetForm::blank`], passing `csrf.token()` from a [`crate::security::CsrfToken`]
//! extractor — the only extra line needed is the parameter itself.
//!
//! # Multipart
//!
//! When the `multipart` feature is enabled, [`ChangesetForm`] also decodes
//! `multipart/form-data` bodies.  File fields are skipped; only text fields
//! are decoded.  File upload storage is out of scope here (see issue #494).
//!
//! # Non-htmx fallback
//!
//! When JavaScript is disabled htmx falls back to a standard form POST.
//! The handler pattern above still works: browsers display the 422 page
//! inline.  For a redirect-after-post pattern, serialise `cs.errors()` into
//! the flash store and redirect; restore on the next GET.

use std::collections::HashMap;

use axum::extract::{FromRequest, Request};
use axum::response::IntoResponse;
use serde::Serialize;

// ── Changeset<T> ───────────────────────────────────────────────────

/// Carries submitted form values and per-field validation errors.
///
/// Analogous to `Ecto.Changeset` in Phoenix or `errors[:field]` in Rails.
///
/// Obtain a `Changeset` from:
/// - [`Changeset::new`] for a blank/valid changeset
/// - [`IntoChangeset::into_changeset`] after manual construction
/// - The [`ChangesetForm`] axum extractor (preferred)
#[derive(Debug)]
pub struct Changeset<T> {
    data: T,
    errors: HashMap<String, Vec<String>>,
}

impl<T> Changeset<T> {
    /// Create a changeset with no errors (valid state).
    pub fn new(data: T) -> Self {
        Self {
            data,
            errors: HashMap::new(),
        }
    }

    /// Create a changeset pre-loaded with field-level errors.
    pub const fn from_errors(data: T, errors: HashMap<String, Vec<String>>) -> Self {
        Self { data, errors }
    }

    /// Returns `true` when there are no field-level errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the validation messages for `field`, or an empty slice.
    pub fn errors_for(&self, field: &str) -> &[String] {
        self.errors.get(field).map_or(&[], Vec::as_slice)
    }

    /// Unwrap the inner data regardless of validity.
    pub fn into_inner(self) -> T {
        self.data
    }

    /// Consume the changeset, returning `Ok(T)` if valid or `Err(self)` if not.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when there are field-level validation errors.
    pub fn into_valid(self) -> Result<T, Self> {
        if self.is_valid() {
            Ok(self.data)
        } else {
            Err(self)
        }
    }

    /// Shared reference to the inner data.
    pub const fn data(&self) -> &T {
        &self.data
    }

    /// All field errors as a map (field name → list of messages).
    pub const fn errors(&self) -> &HashMap<String, Vec<String>> {
        &self.errors
    }
}

impl<T: Serialize> Changeset<T> {
    /// Serialize the value of `field` from the inner data to a `String`.
    ///
    /// Used by rendering helpers to re-populate `<input value="…">` after a
    /// failed submission.  Returns `None` for missing or non-scalar fields.
    pub fn field_value(&self, field: &str) -> Option<String> {
        let json = serde_json::to_value(&self.data).ok()?;
        match json.get(field)? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

// ── IntoChangeset ──────────────────────────────────────────────────

/// Validate `self` and wrap in a [`Changeset`].
///
/// Blanket-implemented for every type that implements [`validator::Validate`].
pub trait IntoChangeset: Sized {
    /// Run validation and produce a `Changeset<Self>`.
    fn into_changeset(self) -> Changeset<Self>;
}

impl<T: validator::Validate> IntoChangeset for T {
    fn into_changeset(self) -> Changeset<Self> {
        match validator::Validate::validate(&self) {
            Ok(()) => Changeset::new(self),
            Err(errors) => Changeset::from_errors(self, validation_errors_to_map(&errors)),
        }
    }
}

// ── ChangesetForm<T> ───────────────────────────────────────────────

/// Axum extractor that decodes a form body, runs validation, and captures the
/// CSRF token — all in one step.
///
/// Supports both `application/x-www-form-urlencoded` (always) and
/// `multipart/form-data` (when the `multipart` feature is enabled).
///
/// Unlike [`crate::validation::Valid`], this extractor **never** rejects with
/// 422 — errors live in the [`Changeset`] and the handler decides how to
/// respond.  Fails with 400 only when the body cannot be decoded into `T` at
/// all.
///
/// # CSRF — no extra developer action in POST handlers
///
/// The extractor reads the `CsrfToken` from request extensions (placed there
/// by [`crate::security::CsrfLayer`]).  Calling
/// [`ChangesetForm::form_tag`] then emits the hidden `_csrf` input
/// automatically — no separate `CsrfToken` parameter needed.
///
/// For GET handlers (new/edit), use [`ChangesetForm::blank`] and pass
/// `csrf.token()` from a `CsrfToken` extractor.
///
/// # Example
///
/// ```rust,ignore
/// #[post("/users")]
/// async fn create(form: ChangesetForm<NewUser>) -> impl IntoResponse {
///     match form.into_valid() {
///         Ok(user) => { /* persist & redirect */ }
///         Err(form) => (StatusCode::UNPROCESSABLE_ENTITY,
///                       form.form_tag("/users", "post", html! {
///                           (form.text_input("name", "Name"))
///                           (form.submit_button("Save"))
///                       })).into_response()
///     }
/// }
/// ```
pub struct ChangesetForm<T> {
    /// The validated (or invalid) changeset.
    pub changeset: Changeset<T>,
    pub(crate) csrf_token: Option<String>,
    pub(crate) csrf_field: String,
}

impl<T> ChangesetForm<T> {
    /// Build a blank form context for GET handlers (new / edit).
    ///
    /// Wraps `data` in a valid [`Changeset`] and stores `csrf_token` so that
    /// [`ChangesetForm::form_tag`] can emit the hidden input automatically.
    ///
    /// ```rust,ignore
    /// #[get("/users/new")]
    /// async fn new_user(csrf: CsrfToken) -> Markup {
    ///     let ctx = ChangesetForm::blank(UserForm::default(), csrf.token());
    ///     ctx.form_tag("/users", "post", html! { (ctx.text_input("name", "Name")) })
    /// }
    /// ```
    pub fn blank(data: T, csrf_token: &str) -> Self {
        Self {
            changeset: Changeset::new(data),
            csrf_token: Some(csrf_token.to_owned()),
            csrf_field: "_csrf".to_owned(),
        }
    }

    /// Construct a display-only `ChangesetForm` with no CSRF token.
    ///
    /// Use this on GET handlers where CSRF middleware is not active, or when
    /// the form will be re-rendered purely for display (e.g. an initial blank
    /// form on a page that does not enforce CSRF).  [`form_tag`](Self::form_tag)
    /// will omit the hidden CSRF input when no token is stored.
    #[must_use]
    pub fn without_csrf(data: T) -> Self {
        Self {
            changeset: Changeset::new(data),
            csrf_token: None,
            csrf_field: "_csrf".to_owned(),
        }
    }

    /// Wrap a pre-built [`Changeset`] (which may already carry validation errors)
    /// in a `ChangesetForm` without a CSRF token.
    ///
    /// Useful in tests and cases where a `Changeset` was produced externally
    /// (e.g. via [`IntoChangeset`]) before constructing a form for rendering.
    #[must_use]
    pub fn from_changeset(changeset: Changeset<T>) -> Self {
        Self {
            changeset,
            csrf_token: None,
            csrf_field: "_csrf".to_owned(),
        }
    }

    /// Override the CSRF form-field name used by [`ChangesetForm::form_tag`].
    ///
    /// Call this when `security.csrf.form_field` is set to something other than
    /// `"_csrf"` (e.g. `"authenticity_token"`).  The `CsrfFormField` extension
    /// populated by [`from_request`](Self::from_request) sets this automatically
    /// for POST handlers; use this builder on GET handlers that construct a blank
    /// form with [`blank`](Self::blank).
    #[must_use]
    pub fn with_csrf_field(mut self, field: impl Into<String>) -> Self {
        self.csrf_field = field.into();
        self
    }

    /// The CSRF token captured from the request, if the CSRF middleware is active.
    pub fn csrf_token(&self) -> Option<&str> {
        self.csrf_token.as_deref()
    }

    /// Consume and return only the inner [`Changeset`].
    pub fn into_changeset(self) -> Changeset<T> {
        self.changeset
    }

    /// Return `Ok(T)` if the changeset is valid, `Err(self)` if not.
    ///
    /// The `Err` branch returns the whole `ChangesetForm` (with its CSRF
    /// token) so the handler can immediately call `form.form_tag()` to
    /// re-render with inline errors.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when the inner changeset has field-level validation errors.
    pub fn into_valid(self) -> Result<T, Self> {
        if self.changeset.is_valid() {
            Ok(self.changeset.into_inner())
        } else {
            Err(self)
        }
    }
}

/// Dereferences to [`Changeset<T>`] so all changeset methods are available
/// directly on `ChangesetForm<T>` — `form.is_valid()`, `form.errors_for(…)`,
/// etc.
impl<T> std::ops::Deref for ChangesetForm<T> {
    type Target = Changeset<T>;
    fn deref(&self) -> &Self::Target {
        &self.changeset
    }
}

/// Maud rendering methods — emit form HTML with automatic CSRF injection.
#[cfg(feature = "maud")]
impl<T: Serialize> ChangesetForm<T> {
    /// Render a `<form>` element with the stored CSRF token injected as a
    /// hidden input — the field name honours `security.csrf.form_field` from
    /// config, so no developer action is required even for non-default names.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn form_tag(&self, action: &str, method: &str, content: maud::Markup) -> maud::Markup {
        form_tag_inner(
            action,
            method,
            &self.csrf_field,
            self.csrf_token.as_deref(),
            content,
        )
    }

    /// Render a labeled `<input type="text">` for `field` using the stored
    /// changeset (value + errors).
    pub fn text_input(&self, field: &str, label: &str) -> maud::Markup {
        text_input(&self.changeset, field, label)
    }

    /// Render a `<button type="submit">` with `label`.
    pub fn submit_button(&self, label: &str) -> maud::Markup {
        submit_button(label)
    }
}

impl<S, T> FromRequest<S> for ChangesetForm<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned + validator::Validate,
{
    type Rejection = axum::response::Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Capture CSRF token and field name before the body is consumed.
        let csrf_token = req
            .extensions()
            .get::<crate::security::CsrfToken>()
            .map(|t| t.token().to_string());
        let csrf_field = req
            .extensions()
            .get::<crate::security::csrf::CsrfFormField>()
            .map_or_else(|| "_csrf".to_owned(), |f| f.0.clone());

        let data: T = decode_form_body(req, state).await?;

        Ok(Self {
            changeset: data.into_changeset(),
            csrf_token,
            csrf_field,
        })
    }
}

/// Decode a form body — URL-encoded always, multipart when that feature is on.
async fn decode_form_body<T, S>(req: Request, state: &S) -> Result<T, axum::response::Response>
where
    T: serde::de::DeserializeOwned + validator::Validate,
    S: Send + Sync,
{
    #[cfg(feature = "multipart")]
    {
        let content_type = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if content_type.starts_with("multipart/form-data") {
            return decode_multipart(req, state).await;
        }
    }

    let axum::extract::Form(data) = axum::extract::Form::<T>::from_request(req, state)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(data)
}

/// Decode `multipart/form-data` text fields and deserialize into `T`.
///
/// File-upload fields are skipped (file storage is out of scope here).
/// The collected text pairs are re-encoded as URL-encoded so that
/// `serde_urlencoded` handles the same type coercions axum's `Form` does.
#[cfg(feature = "multipart")]
async fn decode_multipart<T, S>(req: Request, state: &S) -> Result<T, axum::response::Response>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    let mut multipart = axum::extract::Multipart::from_request(req, state)
        .await
        .map_err(IntoResponse::into_response)?;

    let mut pairs: Vec<(String, String)> = Vec::new();

    loop {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response())?;

        let Some(field) = field else { break };

        let name = match field.name() {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip file-upload fields; text-only decoding is in scope.
        if field.file_name().is_some() {
            continue;
        }

        let value = field
            .text()
            .await
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response())?;

        pairs.push((name, value));
    }

    // Re-encode as URL-encoded so serde_urlencoded handles type coercions
    // ("30" → u32, "true" → bool, etc.) consistently with the Form extractor.
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish();

    serde_urlencoded::from_str::<T>(&encoded)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response())
}

// ── Internal helpers ───────────────────────────────────────────────

fn validation_errors_to_map(errors: &validator::ValidationErrors) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    collect_errors(errors, "", &mut map);
    map
}

fn collect_errors(
    errors: &validator::ValidationErrors,
    prefix: &str,
    map: &mut HashMap<String, Vec<String>>,
) {
    for (field, kind) in errors.errors() {
        let key = if prefix.is_empty() {
            (*field).to_string()
        } else {
            format!("{prefix}.{field}")
        };
        match kind {
            validator::ValidationErrorsKind::Field(errs) => {
                let messages: Vec<String> = errs
                    .iter()
                    .map(|e| {
                        e.message.as_ref().map_or_else(
                            || format!("validation failed: {}", e.code),
                            ToString::to_string,
                        )
                    })
                    .collect();
                map.entry(key).or_default().extend(messages);
            }
            validator::ValidationErrorsKind::Struct(nested) => {
                collect_errors(nested, &key, map);
            }
            validator::ValidationErrorsKind::List(list) => {
                for (idx, nested) in list {
                    let indexed_key = format!("{key}[{idx}]");
                    collect_errors(nested, &indexed_key, map);
                }
            }
        }
    }
}

// ── Standalone Maud helpers ─────────────────────────────────────────
//
// These are the building blocks used by `ChangesetForm` methods.
// They are also public so GET handlers can use them with a bare `Changeset`.

/// Render a `<form>` element wrapping `content`.
///
/// When `csrf_token` is `Some(token)`, a hidden `<input name="_csrf">` is
/// emitted automatically — compatible with [`crate::security::CsrfLayer`]
/// using the default field name `_csrf`.
///
/// In **POST** handlers, prefer [`ChangesetForm::form_tag`] which injects
/// the token **and** honours any custom `security.csrf.form_field` from config.
#[cfg(feature = "maud")]
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn form_tag(
    action: &str,
    method: &str,
    csrf_token: Option<&str>,
    content: maud::Markup,
) -> maud::Markup {
    form_tag_inner(action, method, "_csrf", csrf_token, content)
}

/// Internal: render a `<form>` element using an explicit CSRF field name.
///
/// When `method` is `PUT`, `PATCH`, or `DELETE` (case-insensitive), the
/// browser-facing form method is rewritten to `POST` and a hidden
/// `<input name="_method" value="...">` is emitted so the autumn
/// [`MethodOverrideLayer`](crate::middleware::MethodOverrideLayer) can
/// rewrite the request back to the declared method before route matching.
/// This lets server-rendered HTML target `#[put]` / `#[patch]` /
/// `#[delete]` routes without any client JavaScript.
#[cfg(feature = "maud")]
#[allow(clippy::needless_pass_by_value)]
fn form_tag_inner(
    action: &str,
    method: &str,
    csrf_field: &str,
    csrf_token: Option<&str>,
    content: maud::Markup,
) -> maud::Markup {
    let (browser_method, override_value) = browser_method_and_override(method);
    maud::html! {
        form action=(action) method=(browser_method) {
            @if let Some(override_method) = override_value {
                input
                    type="hidden"
                    name=(crate::middleware::DEFAULT_METHOD_OVERRIDE_FIELD)
                    value=(override_method);
            }
            @if let Some(token) = csrf_token {
                input type="hidden" name=(csrf_field) value=(token);
            }
            (content)
        }
    }
}

/// Translate a declared form method into the browser transport method and
/// any required `_method` override value.
///
/// Returns `(transport, override)` where `override` is `Some(value)` only
/// when the declared method needs a hidden `_method` field.
#[cfg(feature = "maud")]
fn browser_method_and_override(method: &str) -> (&'static str, Option<&'static str>) {
    let trimmed = method.trim();
    if trimmed.eq_ignore_ascii_case("PUT") {
        ("post", Some("PUT"))
    } else if trimmed.eq_ignore_ascii_case("PATCH") {
        ("post", Some("PATCH"))
    } else if trimmed.eq_ignore_ascii_case("DELETE") {
        ("post", Some("DELETE"))
    } else if trimmed.eq_ignore_ascii_case("GET") {
        ("get", None)
    } else {
        ("post", None)
    }
}

/// Render a hidden `<input name="_method" value="...">` field for the
/// declared HTTP method.
///
/// Use this directly when constructing a form by hand (without
/// [`ChangesetForm`] or [`form_tag`]) targeting a `#[put]`, `#[patch]`,
/// or `#[delete]` route from a plain HTML browser submission.
///
/// ```rust,ignore
/// use autumn_web::form::method_input;
///
/// maud::html! {
///     form method="post" action="/posts/42" {
///         (method_input("DELETE"))
///         button { "Delete post" }
///     }
/// }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn method_input(method: &str) -> maud::Markup {
    let normalized = method.trim();
    let value = if normalized.eq_ignore_ascii_case("PUT") {
        "PUT"
    } else if normalized.eq_ignore_ascii_case("PATCH") {
        "PATCH"
    } else if normalized.eq_ignore_ascii_case("DELETE") {
        "DELETE"
    } else {
        // `GET`/`POST` (and anything else) don't need an override — emit
        // nothing rather than producing an invalid override field.
        return maud::html! {};
    };
    maud::html! {
        input
            type="hidden"
            name=(crate::middleware::DEFAULT_METHOD_OVERRIDE_FIELD)
            value=(value);
    }
}

/// Render a labeled `<input type="text">` tied to a changeset field.
///
/// - Sets `name` and `id` to `field`
/// - Populates `value` from the changeset's serialized data
/// - Adds `aria-invalid="true"` + `aria-describedby` when errors exist
/// - Emits a `<div role="alert">` with per-message `<p>` error elements
#[cfg(feature = "maud")]
#[must_use]
pub fn text_input<T: Serialize>(
    changeset: &Changeset<T>,
    field: &str,
    label: &str,
) -> maud::Markup {
    let errors = changeset.errors_for(field);
    let has_errors = !errors.is_empty();
    let value = changeset.field_value(field).unwrap_or_default();
    let error_id = format!("{field}-error");

    maud::html! {
        div {
            label for=(field) { (label) }
            input
                type="text"
                id=(field)
                name=(field)
                value=(value)
                aria-invalid=(if has_errors { "true" } else { "false" })
                aria-describedby=(if has_errors { error_id.as_str() } else { "" });
            @if has_errors {
                div id=(error_id) role="alert" {
                    @for error in errors {
                        p { (error) }
                    }
                }
            }
        }
    }
}

/// Render a `<button type="submit">` with `label`.
#[cfg(feature = "maud")]
#[must_use]
pub fn submit_button(label: &str) -> maud::Markup {
    maud::html! {
        button type="submit" { (label) }
    }
}

/// Render a labeled `<input type="password">` tied to a changeset field.
///
/// Like [`text_input`] but uses `type="password"` and never populates the
/// `value` attribute — browsers must not auto-fill passwords into the markup
/// and screen readers must not announce the value.
///
/// ARIA annotations (`aria-invalid`, `aria-describedby`, error block) behave
/// identically to [`text_input`].
#[cfg(feature = "maud")]
#[must_use]
pub fn password_input<T: Serialize>(
    changeset: &Changeset<T>,
    field: &str,
    label: &str,
) -> maud::Markup {
    let errors = changeset.errors_for(field);
    let has_errors = !errors.is_empty();
    let error_id = format!("{field}-error");

    maud::html! {
        div {
            label for=(field) { (label) }
            input
                type="password"
                id=(field)
                name=(field)
                aria-invalid=(if has_errors { "true" } else { "false" })
                aria-describedby=(if has_errors { error_id.as_str() } else { "" });
            @if has_errors {
                div id=(error_id) role="alert" {
                    @for error in errors {
                        p { (error) }
                    }
                }
            }
        }
    }
}

/// Render a labeled `<textarea>` tied to a changeset field.
///
/// The current field value is emitted as the textarea body (not a `value`
/// attribute). ARIA annotations behave identically to [`text_input`].
#[cfg(feature = "maud")]
#[must_use]
pub fn textarea_input<T: Serialize>(
    changeset: &Changeset<T>,
    field: &str,
    label: &str,
) -> maud::Markup {
    let errors = changeset.errors_for(field);
    let has_errors = !errors.is_empty();
    let value = changeset.field_value(field).unwrap_or_default();
    let error_id = format!("{field}-error");

    maud::html! {
        div {
            label for=(field) { (label) }
            textarea
                id=(field)
                name=(field)
                aria-invalid=(if has_errors { "true" } else { "false" })
                aria-describedby=(if has_errors { error_id.as_str() } else { "" })
                { (value) }
            @if has_errors {
                div id=(error_id) role="alert" {
                    @for error in errors {
                        p { (error) }
                    }
                }
            }
        }
    }
}

/// Render a labeled `<input type="text">` for a required field.
///
/// Identical to [`text_input`] but adds `aria-required="true"` and the HTML
/// `required` attribute, giving both AT users and browser-native validation
/// the required-field signal without relying solely on color.
#[cfg(feature = "maud")]
#[must_use]
pub fn required_text_input<T: Serialize>(
    changeset: &Changeset<T>,
    field: &str,
    label: &str,
) -> maud::Markup {
    let errors = changeset.errors_for(field);
    let has_errors = !errors.is_empty();
    let value = changeset.field_value(field).unwrap_or_default();
    let error_id = format!("{field}-error");

    maud::html! {
        div {
            label for=(field) { (label) }
            input
                type="text"
                id=(field)
                name=(field)
                value=(value)
                required
                aria-required="true"
                aria-invalid=(if has_errors { "true" } else { "false" })
                aria-describedby=(if has_errors { error_id.as_str() } else { "" });
            @if has_errors {
                div id=(error_id) role="alert" {
                    @for error in errors {
                        p { (error) }
                    }
                }
            }
        }
    }
}

/// Render an ARIA live region for htmx swap announcements.
///
/// Emits `<div id="…" role="status" aria-live="polite" aria-atomic="true">`.
/// Place this element in your page layout and update its content via
/// `hx-swap-oob` to announce htmx-driven changes to screen readers without
/// moving keyboard focus.
///
/// # Example
///
/// ```rust,ignore
/// // In your page layout:
/// (aria_live_region("htmx-status", ""))
///
/// // In an htmx response fragment (announces to screen readers):
/// div id="htmx-status" role="status" aria-live="polite" aria-atomic="true"
///     hx-swap-oob="true" {
///     "Post submitted successfully"
/// }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn aria_live_region(id: &str, message: &str) -> maud::Markup {
    maud::html! {
        div id=(id) role="status" aria-live="polite" aria-atomic="true" {
            (message)
        }
    }
}

/// Render a visually-hidden skip-to-content link that becomes visible on focus.
///
/// Place this as the **first element inside `<body>`** so keyboard users can
/// bypass repeated navigation and jump directly to main content.
///
/// The link carries the `skip-link` CSS class; pair it with the bundled
/// Tailwind config's `skip-link` utility or add your own:
///
/// ```css
/// .skip-link { position: absolute; top: -9999px; }
/// .skip-link:focus { position: static; }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn skip_link(target: &str, label: &str) -> maud::Markup {
    maud::html! {
        a href=(target) class="skip-link" { (label) }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Changeset::new ─────────────────────────────────────────────

    #[test]
    fn new_changeset_is_valid() {
        let cs = Changeset::new(42_i32);
        assert!(cs.is_valid());
    }

    #[test]
    fn new_changeset_has_no_errors() {
        let cs = Changeset::new("hello");
        assert!(cs.errors().is_empty());
    }

    #[test]
    fn new_changeset_into_inner() {
        let cs = Changeset::new(99_u8);
        assert_eq!(cs.into_inner(), 99);
    }

    #[test]
    fn new_changeset_data_ref() {
        let cs = Changeset::new(vec![1, 2, 3]);
        assert_eq!(cs.data(), &vec![1, 2, 3]);
    }

    // ── Changeset::from_errors ─────────────────────────────────────

    #[test]
    fn from_errors_changeset_is_invalid() {
        let mut errors = HashMap::new();
        errors.insert("name".to_string(), vec!["too short".to_string()]);
        let cs = Changeset::from_errors("data", errors);
        assert!(!cs.is_valid());
    }

    #[test]
    fn from_errors_returns_correct_field_errors() {
        let mut errors = HashMap::new();
        errors.insert("email".to_string(), vec!["invalid email".to_string()]);
        let cs = Changeset::from_errors("data", errors);
        assert_eq!(cs.errors_for("email"), &["invalid email"]);
    }

    #[test]
    fn errors_for_unknown_field_returns_empty_slice() {
        let cs = Changeset::new("data");
        assert!(cs.errors_for("nonexistent").is_empty());
    }

    #[test]
    fn from_errors_multiple_messages_per_field() {
        let mut errors = HashMap::new();
        errors.insert(
            "password".to_string(),
            vec!["too short".to_string(), "must contain a digit".to_string()],
        );
        let cs = Changeset::from_errors("data", errors);
        let msgs = cs.errors_for("password");
        assert_eq!(msgs.len(), 2);
        assert!(msgs.contains(&"too short".to_string()));
        assert!(msgs.contains(&"must contain a digit".to_string()));
    }

    // ── Changeset::into_valid ──────────────────────────────────────

    #[test]
    fn into_valid_returns_ok_when_valid() {
        let cs = Changeset::new(42_i32);
        assert_eq!(cs.into_valid().expect("should not fail"), 42);
    }

    #[test]
    fn into_valid_returns_err_when_invalid() {
        let mut errors = HashMap::new();
        errors.insert("x".to_string(), vec!["err".to_string()]);
        let cs = Changeset::from_errors(42_i32, errors);
        assert!(cs.into_valid().is_err());
    }

    #[test]
    fn into_valid_err_preserves_changeset() {
        let mut errors = HashMap::new();
        errors.insert("name".to_string(), vec!["required".to_string()]);
        let cs = Changeset::from_errors(7_i32, errors);
        let err_cs = cs.into_valid().unwrap_err();
        assert_eq!(err_cs.into_inner(), 7);
    }

    // ── Changeset::field_value ─────────────────────────────────────

    #[test]
    fn field_value_returns_string_field() {
        #[derive(serde::Serialize)]
        struct Form {
            name: String,
        }
        let cs = Changeset::new(Form {
            name: "Alice".into(),
        });
        assert_eq!(cs.field_value("name"), Some("Alice".to_string()));
    }

    #[test]
    fn field_value_returns_number_as_string() {
        #[derive(serde::Serialize)]
        struct Form {
            age: u32,
        }
        let cs = Changeset::new(Form { age: 30 });
        assert_eq!(cs.field_value("age"), Some("30".to_string()));
    }

    #[test]
    fn field_value_returns_bool_as_string() {
        #[derive(serde::Serialize)]
        struct Form {
            active: bool,
        }
        let cs = Changeset::new(Form { active: true });
        assert_eq!(cs.field_value("active"), Some("true".to_string()));
    }

    #[test]
    fn field_value_returns_none_for_missing_field() {
        #[derive(serde::Serialize)]
        struct Form {
            name: String,
        }
        let cs = Changeset::new(Form {
            name: "Alice".into(),
        });
        assert_eq!(cs.field_value("email"), None);
    }

    #[test]
    fn field_value_after_errors_uses_submitted_data() {
        #[derive(serde::Serialize)]
        struct Form {
            name: String,
        }
        let mut errors = HashMap::new();
        errors.insert("name".to_string(), vec!["too short".to_string()]);
        let cs = Changeset::from_errors(Form { name: "ab".into() }, errors);
        assert_eq!(cs.field_value("name"), Some("ab".to_string()));
    }

    // ── IntoChangeset ──────────────────────────────────────────────

    #[test]
    fn into_changeset_valid_input_produces_no_errors() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 3))]
            name: String,
        }
        let cs = F {
            name: "Alice".into(),
        }
        .into_changeset();
        assert!(cs.is_valid());
        assert!(cs.errors_for("name").is_empty());
    }

    #[test]
    fn into_changeset_invalid_input_populates_errors() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 5))]
            name: String,
        }
        let cs = F { name: "ab".into() }.into_changeset();
        assert!(!cs.is_valid());
        assert!(!cs.errors_for("name").is_empty());
    }

    #[test]
    fn into_changeset_preserves_data_on_failure() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 5))]
            name: String,
        }
        let cs = F { name: "ab".into() }.into_changeset();
        assert_eq!(cs.data().name, "ab");
    }

    #[test]
    fn into_changeset_multiple_fields_errors() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 3))]
            name: String,
            #[validate(email)]
            email: String,
        }
        let cs = F {
            name: "a".into(),
            email: "not-email".into(),
        }
        .into_changeset();
        assert!(!cs.is_valid());
        assert!(!cs.errors_for("name").is_empty());
        assert!(!cs.errors_for("email").is_empty());
    }

    mod nested_validation {
        use super::*;
        use validator::Validate as _;

        #[derive(validator::Validate)]
        struct NestedAddress {
            #[validate(length(min = 3, message = "street too short"))]
            street: String,
        }

        #[derive(validator::Validate)]
        struct PersonWithAddress {
            #[validate(nested)]
            address: NestedAddress,
        }

        #[test]
        fn nested_struct_errors_are_flattened_with_dot_notation() {
            let cs = PersonWithAddress {
                address: NestedAddress { street: "x".into() },
            }
            .into_changeset();
            assert!(!cs.is_valid());
            assert!(!cs.errors_for("address.street").is_empty());
        }
    }

    // ── ChangesetForm helpers ──────────────────────────────────────

    #[test]
    fn changeset_form_blank_is_valid() {
        #[derive(validator::Validate, serde::Serialize)]
        struct F {
            #[validate(length(min = 1))]
            name: String,
        }
        let form = ChangesetForm::blank(F { name: "ok".into() }, "tok");
        assert!(form.is_valid()); // via Deref
        assert_eq!(form.csrf_token(), Some("tok"));
    }

    #[test]
    fn changeset_form_deref_exposes_changeset_methods() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 3))]
            name: String,
        }
        let changeset = F { name: "ab".into() }.into_changeset();
        let form = ChangesetForm {
            changeset,
            csrf_token: None,
            csrf_field: "_csrf".into(),
        };
        // Deref gives access to Changeset methods
        assert!(!form.is_valid());
        assert!(!form.errors_for("name").is_empty());
    }

    #[test]
    fn changeset_form_into_valid_ok() {
        #[derive(validator::Validate)]
        struct F {
            #[validate(length(min = 1))]
            name: String,
        }
        let form = ChangesetForm {
            changeset: F { name: "ok".into() }.into_changeset(),
            csrf_token: None,
            csrf_field: "_csrf".into(),
        };
        assert!(form.into_valid().is_ok());
    }

    #[test]
    fn changeset_form_into_valid_err_preserves_csrf() {
        #[derive(Debug, validator::Validate)]
        struct F {
            #[validate(length(min = 5))]
            name: String,
        }
        let form = ChangesetForm {
            changeset: F { name: "ab".into() }.into_changeset(),
            csrf_token: Some("tok123".into()),
            csrf_field: "_csrf".into(),
        };
        let err_form = form.into_valid().unwrap_err();
        assert_eq!(err_form.csrf_token(), Some("tok123"));
    }

    // ── Maud helpers ───────────────────────────────────────────────

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_renders_action_and_method() {
        let html = form_tag("/users", "post", None, maud::html! { "" }).into_string();
        assert!(html.contains(r#"action="/users""#), "{html}");
        assert!(html.contains(r#"method="post""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_emits_csrf_hidden_input_when_token_provided() {
        let html = form_tag("/users", "post", Some("tok123"), maud::html! { "" }).into_string();
        assert!(html.contains(r#"name="_csrf""#), "{html}");
        assert!(html.contains(r#"value="tok123""#), "{html}");
        assert!(html.contains(r#"type="hidden""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_omits_csrf_input_when_none() {
        let html = form_tag("/users", "post", None, maud::html! { "" }).into_string();
        assert!(!html.contains("_csrf"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_includes_content() {
        let html = form_tag("/x", "post", None, maud::html! { span { "inner" } }).into_string();
        assert!(html.contains("inner"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_emits_method_override_for_delete() {
        let html = form_tag("/posts/42", "delete", None, maud::html! { "" }).into_string();
        // Browser-facing method must be POST so native form submission works.
        assert!(html.contains(r#"method="post""#), "{html}");
        assert!(!html.contains(r#"method="delete""#), "{html}");
        // Hidden override field tells the autumn middleware to rewrite to DELETE.
        assert!(html.contains(r#"name="_method""#), "{html}");
        assert!(html.contains(r#"value="DELETE""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_emits_method_override_for_put_and_patch() {
        let put_html = form_tag("/p/1", "put", None, maud::html! { "" }).into_string();
        assert!(put_html.contains(r#"method="post""#));
        assert!(put_html.contains(r#"value="PUT""#));

        let patch_html = form_tag("/p/1", "PATCH", None, maud::html! { "" }).into_string();
        assert!(patch_html.contains(r#"method="post""#));
        assert!(patch_html.contains(r#"value="PATCH""#));
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_no_override_for_get_or_post() {
        let get_html = form_tag("/p", "get", None, maud::html! { "" }).into_string();
        assert!(!get_html.contains("_method"), "{get_html}");
        let post_html = form_tag("/p", "post", None, maud::html! { "" }).into_string();
        assert!(!post_html.contains("_method"), "{post_html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn method_input_emits_hidden_field_for_mutating_methods() {
        for method in ["PUT", "PATCH", "DELETE", "delete"] {
            let html = method_input(method).into_string();
            assert!(html.contains(r#"name="_method""#), "{html}");
            assert!(html.contains(r#"type="hidden""#), "{html}");
        }
    }

    #[cfg(feature = "maud")]
    #[test]
    fn method_input_is_empty_for_safe_or_unknown_methods() {
        assert_eq!(method_input("GET").into_string(), "");
        assert_eq!(method_input("POST").into_string(), "");
        assert_eq!(method_input("BREW").into_string(), "");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn changeset_form_form_tag_injects_stored_csrf() {
        #[derive(validator::Validate, serde::Serialize)]
        struct F {
            name: String,
        }
        let form = ChangesetForm::blank(
            F {
                name: String::new(),
            },
            "secret-token",
        );
        let html = form
            .form_tag("/x", "post", maud::html! { "" })
            .into_string();
        assert!(html.contains(r#"value="secret-token""#), "{html}");
        assert!(html.contains(r#"name="_csrf""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn changeset_form_form_tag_honours_custom_csrf_field_name() {
        #[derive(validator::Validate, serde::Serialize)]
        struct F {
            name: String,
        }
        let form = ChangesetForm {
            changeset: Changeset::new(F {
                name: String::new(),
            }),
            csrf_token: Some("tok".into()),
            csrf_field: "authenticity_token".into(),
        };
        let html = form
            .form_tag("/x", "post", maud::html! { "" })
            .into_string();
        assert!(html.contains(r#"name="authenticity_token""#), "{html}");
        assert!(!html.contains(r#"name="_csrf""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn text_input_renders_label_name_and_value() {
        #[derive(serde::Serialize)]
        struct F {
            name: String,
        }
        let cs = Changeset::new(F {
            name: "Alice".into(),
        });
        let html = text_input(&cs, "name", "Full Name").into_string();
        assert!(html.contains(r#"name="name""#), "{html}");
        assert!(html.contains(r#"value="Alice""#), "{html}");
        assert!(html.contains("Full Name"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn text_input_aria_invalid_false_when_no_errors() {
        #[derive(serde::Serialize)]
        struct F {
            name: String,
        }
        let cs = Changeset::new(F {
            name: "Alice".into(),
        });
        let html = text_input(&cs, "name", "Name").into_string();
        assert!(html.contains(r#"aria-invalid="false""#), "{html}");
        assert!(!html.contains(r#"role="alert""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn text_input_aria_invalid_true_and_error_block_on_failure() {
        #[derive(serde::Serialize)]
        struct F {
            name: String,
        }
        let mut errors = HashMap::new();
        errors.insert("name".to_string(), vec!["too short".to_string()]);
        let cs = Changeset::from_errors(F { name: "ab".into() }, errors);
        let html = text_input(&cs, "name", "Name").into_string();
        assert!(html.contains(r#"aria-invalid="true""#), "{html}");
        assert!(html.contains(r#"role="alert""#), "{html}");
        assert!(html.contains("too short"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn text_input_error_block_has_describedby_link() {
        #[derive(serde::Serialize)]
        struct F {
            email: String,
        }
        let mut errors = HashMap::new();
        errors.insert("email".to_string(), vec!["invalid".to_string()]);
        let cs = Changeset::from_errors(F { email: "x".into() }, errors);
        let html = text_input(&cs, "email", "Email").into_string();
        assert!(html.contains("email-error"), "{html}");
        assert!(html.contains(r#"aria-describedby="email-error""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn text_input_multiple_errors_all_rendered() {
        #[derive(serde::Serialize)]
        struct F {
            password: String,
        }
        let mut errors = HashMap::new();
        errors.insert(
            "password".to_string(),
            vec!["too short".to_string(), "needs digit".to_string()],
        );
        let cs = Changeset::from_errors(
            F {
                password: "x".into(),
            },
            errors,
        );
        let html = text_input(&cs, "password", "Password").into_string();
        assert!(html.contains("too short"), "{html}");
        assert!(html.contains("needs digit"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn submit_button_renders_button_with_label() {
        let html = submit_button("Save").into_string();
        assert!(html.contains(r#"type="submit""#), "{html}");
        assert!(html.contains("Save"), "{html}");
    }

    // ── RED: accessible form helpers ───────────────────────────────

    #[cfg(feature = "maud")]
    #[test]
    fn password_input_renders_type_password() {
        #[derive(serde::Serialize)]
        struct F {
            password: String,
        }
        let cs = Changeset::new(F {
            password: String::new(),
        });
        let html = password_input(&cs, "password", "Password").into_string();
        assert!(html.contains(r#"type="password""#), "{html}");
        assert!(html.contains(r#"name="password""#), "{html}");
        assert!(html.contains("Password"), "{html}");
        // Must NOT expose the value in the rendered HTML
        assert!(!html.contains(r#"value=""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn password_input_emits_aria_invalid_on_error() {
        #[derive(serde::Serialize)]
        struct F {
            password: String,
        }
        let mut errors = HashMap::new();
        errors.insert("password".to_string(), vec!["too short".to_string()]);
        let cs = Changeset::from_errors(
            F {
                password: "x".into(),
            },
            errors,
        );
        let html = password_input(&cs, "password", "Password").into_string();
        assert!(html.contains(r#"aria-invalid="true""#), "{html}");
        assert!(html.contains(r#"role="alert""#), "{html}");
        assert!(html.contains("too short"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn textarea_input_renders_textarea_element() {
        #[derive(serde::Serialize)]
        struct F {
            bio: String,
        }
        let cs = Changeset::new(F {
            bio: "Hello world".into(),
        });
        let html = textarea_input(&cs, "bio", "Bio").into_string();
        assert!(html.contains("<textarea"), "{html}");
        assert!(html.contains(r#"name="bio""#), "{html}");
        assert!(html.contains(r#"id="bio""#), "{html}");
        assert!(html.contains("Bio"), "{html}");
        assert!(html.contains("Hello world"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn textarea_input_aria_invalid_on_error() {
        #[derive(serde::Serialize)]
        struct F {
            bio: String,
        }
        let mut errors = HashMap::new();
        errors.insert("bio".to_string(), vec!["required".to_string()]);
        let cs = Changeset::from_errors(F { bio: String::new() }, errors);
        let html = textarea_input(&cs, "bio", "Bio").into_string();
        assert!(html.contains(r#"aria-invalid="true""#), "{html}");
        assert!(html.contains(r#"role="alert""#), "{html}");
        assert!(html.contains("required"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn required_text_input_emits_aria_required() {
        #[derive(serde::Serialize)]
        struct F {
            name: String,
        }
        let cs = Changeset::new(F {
            name: "Alice".into(),
        });
        let html = required_text_input(&cs, "name", "Name").into_string();
        assert!(html.contains(r#"aria-required="true""#), "{html}");
        assert!(html.contains("required"), "{html}");
        assert!(html.contains(r#"name="name""#), "{html}");
        assert!(html.contains("Name"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn required_text_input_preserves_error_handling() {
        #[derive(serde::Serialize)]
        struct F {
            name: String,
        }
        let mut errors = HashMap::new();
        errors.insert("name".to_string(), vec!["required".to_string()]);
        let cs = Changeset::from_errors(
            F {
                name: String::new(),
            },
            errors,
        );
        let html = required_text_input(&cs, "name", "Name").into_string();
        assert!(html.contains(r#"aria-invalid="true""#), "{html}");
        assert!(html.contains(r#"aria-required="true""#), "{html}");
        assert!(html.contains(r#"role="alert""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn aria_live_region_renders_role_status() {
        let html = aria_live_region("status-msg", "").into_string();
        assert!(html.contains(r#"role="status""#), "{html}");
        assert!(html.contains(r#"aria-live="polite""#), "{html}");
        assert!(html.contains(r#"id="status-msg""#), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn aria_live_region_renders_message_content() {
        let html = aria_live_region("status-msg", "Form submitted").into_string();
        assert!(html.contains("Form submitted"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn skip_link_renders_anchor_with_href() {
        let html = skip_link("#main-content", "Skip to main content").into_string();
        assert!(html.contains(r##"href="#main-content""##), "{html}");
        assert!(html.contains("Skip to main content"), "{html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn skip_link_has_visually_hidden_class_for_focus_reveal() {
        let html = skip_link("#main", "Skip").into_string();
        assert!(html.contains("skip-link"), "{html}");
    }

    // ── ChangesetForm extractor (axum integration) ─────────────────

    mod extractor_tests {
        use super::*;
        use axum::{Router, body::Body, routing::post};
        use tower::ServiceExt;

        #[derive(serde::Deserialize, validator::Validate)]
        struct TestForm {
            #[validate(length(min = 3))]
            name: String,
        }

        #[tokio::test]
        async fn valid_form_body_produces_valid_changeset() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                format!("valid={}", form.is_valid())
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(urlencoded_req("/test", "name=Alice"))
                .await
                .expect("should not fail");
            assert_body(resp, "valid=true").await;
        }

        #[tokio::test]
        async fn invalid_form_body_produces_invalid_changeset() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                format!("valid={}", form.is_valid())
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(urlencoded_req("/test", "name=ab"))
                .await
                .expect("should not fail");
            assert_body(resp, "valid=false").await;
        }

        #[tokio::test]
        async fn invalid_form_exposes_field_errors() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                form.errors_for("name").join("|")
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(urlencoded_req("/test", "name=ab"))
                .await
                .expect("should not fail");
            let body = body_text(resp).await;
            assert!(!body.is_empty(), "expected errors, got empty string");
        }

        #[tokio::test]
        async fn missing_required_field_returns_non_200() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                format!("valid={}", form.is_valid())
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(urlencoded_req("/test", "other=value"))
                .await
                .expect("should not fail");
            assert_ne!(resp.status(), axum::http::StatusCode::OK);
        }

        #[tokio::test]
        async fn csrf_token_is_none_without_csrf_middleware() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                form.csrf_token().unwrap_or("none").to_string()
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(urlencoded_req("/test", "name=Alice"))
                .await
                .expect("should not fail");
            assert_body(resp, "none").await;
        }

        #[tokio::test]
        async fn csrf_token_captured_from_request_extensions() {
            // Build a request with CsrfToken pre-inserted in extensions,
            // simulating what CsrfLayer does, then call from_request directly.
            use crate::security::CsrfToken;

            let mut req = axum::http::Request::builder()
                .method("POST")
                .uri("/test")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Alice"))
                .expect("should not fail");
            req.extensions_mut()
                .insert(CsrfToken::new("secret-tok".to_string()));

            let form = ChangesetForm::<TestForm>::from_request(req, &())
                .await
                .expect("extraction should succeed");

            assert_eq!(form.csrf_token(), Some("secret-tok"));
        }

        #[cfg(feature = "multipart")]
        #[tokio::test]
        async fn multipart_form_decodes_text_fields() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                format!("valid={} name={}", form.is_valid(), form.data().name)
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(multipart_req("/test", "name", "Alice"))
                .await
                .expect("should not fail");
            assert_body(resp, "valid=true name=Alice").await;
        }

        #[cfg(feature = "multipart")]
        #[tokio::test]
        async fn multipart_form_validates_fields() {
            async fn handler(form: ChangesetForm<TestForm>) -> String {
                format!("valid={}", form.is_valid())
            }
            let resp = Router::new()
                .route("/test", post(handler))
                .oneshot(multipart_req("/test", "name", "ab"))
                .await
                .expect("should not fail");
            assert_body(resp, "valid=false").await;
        }

        // ── Helpers ────────────────────────────────────────────────

        fn urlencoded_req(uri: &str, body: &'static str) -> axum::http::Request<Body> {
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .expect("should not fail")
        }

        #[cfg(feature = "multipart")]
        fn multipart_req(uri: &str, field: &str, value: &str) -> axum::http::Request<Body> {
            let boundary = "----FormBoundary7MA4YWxkTrZu0gW";
            let body = format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"{field}\"\r\n\r\n\
                 {value}\r\n\
                 --{boundary}--\r\n"
            );
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("should not fail")
        }

        async fn body_text(resp: axum::response::Response) -> String {
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("should not fail");
            String::from_utf8(bytes.to_vec()).expect("should not fail")
        }

        async fn assert_body(resp: axum::response::Response, expected: &str) {
            assert_eq!(body_text(resp).await, expected);
        }
    }
}
