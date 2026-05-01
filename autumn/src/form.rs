//! Changeset-style form helpers with validation and Maud rendering.
//!
//! # Overview
//!
//! [`Changeset<T>`] captures submitted form values together with per-field
//! validation errors, enabling the create/edit/validate-failure round-trip
//! in a single route handler — no manual flash-carrying, no conditional
//! error-threading.
//!
//! # Framework comparison
//!
//! | Framework | Changeset type | Rendering helper |
//! |-----------|---------------|-----------------|
//! | Phoenix (Elixir) | `Ecto.Changeset` | `<.input field={@form[:name]} />` |
//! | Rails (Ruby) | `errors[:field]` | `f.text_field :name` |
//! | Django (Python) | `forms.Form` | `{{ form.name.errors }}` |
//! | Autumn (Rust) | `Changeset<T>` | `text_input(&cs, "name", "Name")` |
//!
//! # Happy-path + validation-failure in one handler
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::form::{ChangesetForm, Changeset, form_tag, text_input, submit_button};
//! use serde::{Deserialize, Serialize};
//! use validator::Validate;
//! use axum::{http::StatusCode, response::IntoResponse};
//!
//! #[derive(Deserialize, Serialize, Validate)]
//! struct CreateUser {
//!     #[validate(length(min = 3, max = 50))]
//!     name: String,
//!     #[validate(email)]
//!     email: String,
//! }
//!
//! fn user_form(cs: &Changeset<CreateUser>, csrf: &str, action: &str) -> Markup {
//!     form_tag(action, "post", Some(csrf), html! {
//!         (text_input(cs, "name", "Full name"))
//!         (text_input(cs, "email", "Email address"))
//!         (submit_button("Save"))
//!     })
//! }
//!
//! #[get("/users/new")]
//! async fn new_user(csrf: CsrfToken) -> Markup {
//!     let blank = Changeset::new(CreateUser { name: String::new(), email: String::new() });
//!     user_form(&blank, csrf.token(), "/users")
//! }
//!
//! // htmx round-trip: on failure returns 422 + the same partial.
//! // Non-htmx: replace `(StatusCode::UNPROCESSABLE_ENTITY, ...)` with
//! // a redirect after storing the changeset in flash.
//! #[post("/users")]
//! async fn create_user(
//!     csrf: CsrfToken,
//!     ChangesetForm(cs): ChangesetForm<CreateUser>,
//! ) -> impl IntoResponse {
//!     match cs.into_valid() {
//!         Ok(_user) => Redirect::to("/users").into_response(),
//!         Err(cs) => (
//!             StatusCode::UNPROCESSABLE_ENTITY,
//!             user_form(&cs, csrf.token(), "/users"),
//!         )
//!             .into_response(),
//!     }
//! }
//! ```
//!
//! # Non-htmx / progressive-enhancement fallback
//!
//! When JavaScript is disabled htmx falls back to a normal form POST.  The
//! handler above already works — it returns a full-page response with a 422
//! status that browsers display inline.  If you prefer the classic
//! redirect-after-post pattern to avoid double-submit on refresh, store the
//! changeset in the session via [`crate::flash`] and redirect:
//!
//! ```rust,ignore
//! // On failure: serialize errors + values into flash, redirect back.
//! flash.error(serde_json::to_string(&cs.errors()).unwrap()).await;
//! Redirect::to("/users/new").into_response()
//! ```
//!
//! # CSRF
//!
//! Pass `Some(csrf.token())` to [`form_tag`] and the hidden `_csrf` input is
//! emitted automatically.  No additional developer action is needed when the
//! [`crate::security::CsrfLayer`] middleware is active.

use std::collections::HashMap;

use axum::extract::{FromRequest, Request};
use axum::response::IntoResponse;
use serde::Serialize;

// ── Changeset<T> ───────────────────────────────────────────────────

/// Carries submitted form values and per-field validation errors.
///
/// A `Changeset` is the central type for form handling in Autumn.  It holds
/// the decoded input (even when invalid) alongside any field-level validation
/// messages, letting a single handler both validate *and* re-render the form
/// with the user's previous values and inline error messages.
///
/// # Obtaining a Changeset
///
/// | Source | When to use |
/// |--------|-------------|
/// | [`Changeset::new`] | Blank form (GET handler) |
/// | [`IntoChangeset::into_changeset`] | Explicit validation after manual construction |
/// | [`ChangesetForm`] extractor | POST handler — decodes body + validates |
#[derive(Debug)]
pub struct Changeset<T> {
    data: T,
    errors: HashMap<String, Vec<String>>,
}

impl<T> Changeset<T> {
    /// Create a changeset with no errors (valid state).
    pub fn new(data: T) -> Self {
        todo!("red phase")
    }

    /// Create a changeset pre-loaded with field-level errors.
    pub fn from_errors(data: T, errors: HashMap<String, Vec<String>>) -> Self {
        todo!("red phase")
    }

    /// Returns `true` when there are no field-level errors.
    pub fn is_valid(&self) -> bool {
        todo!("red phase")
    }

    /// Returns the validation messages for `field`, or an empty slice.
    pub fn errors_for(&self, field: &str) -> &[String] {
        todo!("red phase")
    }

    /// Unwrap the inner data regardless of validity.
    pub fn into_inner(self) -> T {
        todo!("red phase")
    }

    /// Consume the changeset, returning `Ok(T)` if valid or `Err(self)` if not.
    ///
    /// The `Err` branch gives back the changeset so the handler can pass it
    /// to a Maud rendering function.
    pub fn into_valid(self) -> Result<T, Self> {
        todo!("red phase")
    }

    /// Shared reference to the inner data.
    pub fn data(&self) -> &T {
        todo!("red phase")
    }

    /// All field errors as a map (field name → list of messages).
    pub fn errors(&self) -> &HashMap<String, Vec<String>> {
        todo!("red phase")
    }
}

impl<T: Serialize> Changeset<T> {
    /// Serialize the value of `field` from the inner data to a `String`.
    ///
    /// Used by [`text_input`] to re-populate `<input value="…">` after a
    /// failed submission so the user does not lose their typed input.
    ///
    /// Returns `None` when the field does not exist or cannot be represented
    /// as a plain string (e.g., nested objects, arrays).
    pub fn field_value(&self, field: &str) -> Option<String> {
        todo!("red phase")
    }
}

// ── IntoChangeset ──────────────────────────────────────────────────

/// Extension trait that validates `self` and wraps the result in a [`Changeset`].
///
/// Blanket-implemented for every type that implements [`validator::Validate`].
pub trait IntoChangeset: Sized {
    /// Run validation and produce a `Changeset<Self>`.
    ///
    /// On success the changeset has no errors (`is_valid() == true`).
    /// On failure each failed field's messages appear in `errors_for(field)`.
    fn into_changeset(self) -> Changeset<Self>;
}

impl<T: validator::Validate> IntoChangeset for T {
    fn into_changeset(self) -> Changeset<Self> {
        todo!("red phase")
    }
}

// ── ChangesetForm<T> extractor ─────────────────────────────────────

/// Axum extractor that decodes a URL-encoded form body and runs validation.
///
/// Unlike [`crate::validation::Valid`], this extractor **never** returns a
/// 422 on its own — validation errors are captured inside the [`Changeset`]
/// and the decision of how to respond is left to the handler.  This enables
/// the inline-error pattern without littering routes with custom rejection
/// handling.
///
/// # Failure modes
///
/// | Condition | HTTP status |
/// |-----------|------------|
/// | Body cannot be decoded into `T` | 400 Bad Request |
/// | Validation fails | 200 — errors in `Changeset` |
///
/// # Example
///
/// ```rust,ignore
/// #[post("/users")]
/// async fn create(ChangesetForm(cs): ChangesetForm<NewUser>) -> impl IntoResponse {
///     match cs.into_valid() {
///         Ok(user) => { /* persist & redirect */ }
///         Err(cs)  => (StatusCode::UNPROCESSABLE_ENTITY, form_view(&cs)).into_response()
///     }
/// }
/// ```
pub struct ChangesetForm<T>(pub Changeset<T>);

impl<S, T> FromRequest<S> for ChangesetForm<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned + validator::Validate,
{
    type Rejection = axum::response::Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        todo!("red phase")
    }
}

// ── Internal helpers ───────────────────────────────────────────────

fn validation_errors_to_map(
    errors: &validator::ValidationErrors,
) -> HashMap<String, Vec<String>> {
    todo!("red phase")
}

// ── Maud rendering helpers ──────────────────────────────────────────

/// Render a `<form>` element wrapping `content`.
///
/// When `csrf_token` is `Some(token)`, a hidden `<input name="_csrf">` is
/// emitted automatically inside the opening tag — compatible with
/// [`crate::security::CsrfLayer`]'s form-field validation strategy.
///
/// # Example
///
/// ```rust,ignore
/// form_tag("/users", "post", Some(csrf.token()), html! {
///     (text_input(&cs, "name", "Full name"))
///     (submit_button("Save"))
/// })
/// ```
#[cfg(feature = "maud")]
pub fn form_tag(
    action: &str,
    method: &str,
    csrf_token: Option<&str>,
    content: maud::Markup,
) -> maud::Markup {
    todo!("red phase")
}

/// Render a labeled `<input type="text">` tied to a changeset field.
///
/// Automatically:
/// - sets `name` and `id` to `field`
/// - populates `value` from the changeset's serialized inner data
/// - adds `aria-invalid="true"` and `aria-describedby` when errors exist
/// - emits a sibling `<div role="alert">` with the error messages
///
/// # Accessibility
///
/// The error block carries `role="alert"` so screen readers announce the
/// message immediately after re-render.  The `aria-describedby` on the
/// input links the two elements so assistive technology can also discover
/// the error via the input's description.
#[cfg(feature = "maud")]
pub fn text_input<T: Serialize>(
    changeset: &Changeset<T>,
    field: &str,
    label: &str,
) -> maud::Markup {
    todo!("red phase")
}

/// Render a `<button type="submit">` with `label`.
#[cfg(feature = "maud")]
pub fn submit_button(label: &str) -> maud::Markup {
    todo!("red phase")
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
        assert_eq!(cs.into_valid().unwrap(), 42);
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
        // Even though invalid, field_value returns the submitted (bad) value
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
        let f = F {
            name: "Alice".into(),
        };
        let cs = f.into_changeset();
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
        let f = F { name: "ab".into() };
        let cs = f.into_changeset();
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
        let f = F { name: "ab".into() };
        let cs = f.into_changeset();
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
        let f = F {
            name: "a".into(),
            email: "not-email".into(),
        };
        let cs = f.into_changeset();
        assert!(!cs.is_valid());
        assert!(!cs.errors_for("name").is_empty());
        assert!(!cs.errors_for("email").is_empty());
    }

    // ── Maud helpers ───────────────────────────────────────────────

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_renders_action_and_method() {
        let markup = form_tag("/users", "post", None, maud::html! { "" });
        let html = markup.into_string();
        assert!(html.contains(r#"action="/users""#), "missing action: {html}");
        assert!(html.contains(r#"method="post""#), "missing method: {html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_emits_csrf_hidden_input_when_token_provided() {
        let markup = form_tag("/users", "post", Some("tok123"), maud::html! { "" });
        let html = markup.into_string();
        assert!(html.contains(r#"name="_csrf""#), "missing _csrf: {html}");
        assert!(html.contains(r#"value="tok123""#), "missing token value: {html}");
        assert!(
            html.contains(r#"type="hidden""#),
            "missing hidden type: {html}"
        );
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_omits_csrf_input_when_none() {
        let markup = form_tag("/users", "post", None, maud::html! { "" });
        let html = markup.into_string();
        assert!(!html.contains("_csrf"), "unexpected _csrf: {html}");
    }

    #[cfg(feature = "maud")]
    #[test]
    fn form_tag_includes_content() {
        let markup = form_tag(
            "/x",
            "post",
            None,
            maud::html! { span { "inner content" } },
        );
        let html = markup.into_string();
        assert!(html.contains("inner content"), "missing content: {html}");
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
        let cs = Changeset::from_errors(F { password: "x".into() }, errors);
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

    // ── ChangesetForm extractor ────────────────────────────────────

    #[cfg(test)]
    mod extractor_tests {
        use super::*;
        use axum::{body::Body, http::Request, routing::post, Router};
        use tower::ServiceExt;

        #[derive(serde::Deserialize, validator::Validate)]
        struct TestForm {
            #[validate(length(min = 3))]
            name: String,
        }

        #[tokio::test]
        async fn valid_form_body_produces_valid_changeset() {
            async fn handler(ChangesetForm(cs): ChangesetForm<TestForm>) -> String {
                format!("valid={}", cs.is_valid())
            }

            let app = Router::new().route("/test", post(handler));
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/test")
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("name=Alice"))
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(std::str::from_utf8(&body).unwrap(), "valid=true");
        }

        #[tokio::test]
        async fn invalid_form_body_produces_invalid_changeset() {
            async fn handler(ChangesetForm(cs): ChangesetForm<TestForm>) -> String {
                format!("valid={}", cs.is_valid())
            }

            let app = Router::new().route("/test", post(handler));
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/test")
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("name=ab")) // too short
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(std::str::from_utf8(&body).unwrap(), "valid=false");
        }

        #[tokio::test]
        async fn invalid_form_exposes_field_errors_via_changeset() {
            async fn handler(ChangesetForm(cs): ChangesetForm<TestForm>) -> String {
                cs.errors_for("name").join("|")
            }

            let app = Router::new().route("/test", post(handler));
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/test")
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("name=ab"))
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = std::str::from_utf8(&body).unwrap();
            assert!(!text.is_empty(), "expected error messages, got empty string");
        }

        #[tokio::test]
        async fn missing_required_field_returns_non_200() {
            async fn handler(ChangesetForm(cs): ChangesetForm<TestForm>) -> String {
                format!("valid={}", cs.is_valid())
            }

            let app = Router::new().route("/test", post(handler));
            // Body with no `name` key — serde deserialization fails
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/test")
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("other=value"))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_ne!(
                resp.status(),
                axum::http::StatusCode::OK,
                "expected non-200 for failed decode"
            );
        }
    }
}
