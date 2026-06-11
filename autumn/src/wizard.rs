//! First-class multi-step form wizards with session-backed state.
//!
//! Orchestrates multi-step flows (onboarding, checkout, KYC) on top of the
//! existing [`Session`], [`Changeset`], and [`Flash`] primitives — no new
//! storage machinery.
//!
//! ## Session key format
//!
//! Each step's data is stored under:
//! ```text
//! __autumn_wizard:{wizard_name}:{step_name}
//! ```
//! Values are serde-JSON strings, so they round-trip through any
//! `SessionStore` backend (memory or Redis) without change.
//!
//! ## Quick start
//!
//! ```rust,ignore
//! use autumn_web::wizard::{WizardContext, wizard_progress};
//!
//! // In a GET handler for step 1:
//! #[get("/checkout/shipping")]
//! async fn shipping_step(session: Session, csrf: CsrfToken) -> impl IntoResponse {
//!     let wizard = WizardContext::new(session, "checkout",
//!         vec!["shipping".into(), "payment".into(), "review".into()]);
//!     let data = wizard.step_data::<ShippingForm>("shipping").await
//!         .unwrap_or_default();
//!     let form = ChangesetForm::blank(data, csrf.token());
//!     html! {
//!         (wizard_progress(&wizard, "shipping").await)
//!         (form.form_tag("/checkout/shipping", "post", html! {
//!             (form.text_input("address", "Address"))
//!             (form.submit_button("Continue"))
//!         }))
//!     }
//! }
//!
//! // In a POST handler:
//! #[post("/checkout/shipping")]
//! async fn shipping_submit(session: Session, form: ChangesetForm<ShippingForm>)
//!     -> impl IntoResponse
//! {
//!     let wizard = WizardContext::new(session, "checkout",
//!         vec!["shipping".into(), "payment".into(), "review".into()]);
//!     match form.into_valid() {
//!         Ok(data) => {
//!             wizard.save_step("shipping", &data).await.unwrap();
//!             Redirect::to("/checkout/payment")
//!         }
//!         Err(form) => (StatusCode::UNPROCESSABLE_ENTITY, render_form(&form)).into_response(),
//!     }
//! }
//! ```

use serde::{Serialize, de::DeserializeOwned};

use crate::session::Session;

#[cfg(feature = "maud")]
use maud::{Markup, html};

/// Error type for wizard operations.
#[derive(Debug, thiserror::Error)]
pub enum WizardError {
    /// Failed to serialize step data to JSON.
    #[error("failed to serialize wizard step data: {0}")]
    Serialize(#[from] serde_json::Error),
}

// Session key prefix for wizard state.
const WIZARD_KEY_PREFIX: &str = "__autumn_wizard";

/// Context for a multi-step wizard flow.
///
/// Holds the session, wizard name, and ordered step list. Provides methods
/// for persisting per-step data, checking completion state, and performing
/// step-guard redirects.
///
/// Construct in each handler and pass around; it wraps the [`Session`] by
/// reference so it shares the same session handle.
#[derive(Clone, Debug)]
pub struct WizardContext {
    session: Session,
    name: String,
    steps: Vec<String>,
}

impl WizardContext {
    /// Create a new wizard context.
    ///
    /// `name` identifies the wizard (e.g. `"checkout"`).
    /// `steps` is the ordered list of step names (e.g. `["shipping", "payment", "review"]`).
    pub fn new(
        session: Session,
        name: impl Into<String>,
        steps: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            session,
            name: name.into(),
            steps: steps.into_iter().collect(),
        }
    }

    /// The total number of steps in this wizard.
    pub fn total_steps(&self) -> usize {
        self.steps.len()
    }

    /// The 0-based index of the named step, or `None` if not found.
    pub fn step_index(&self, step: &str) -> Option<usize> {
        self.steps.iter().position(|s| s == step)
    }

    /// The 1-based step number for display (e.g. `"step 2 of 4"`), or `None`.
    pub fn step_number(&self, step: &str) -> Option<usize> {
        self.step_index(step).map(|i| i + 1)
    }

    /// An ordered slice of all step names.
    pub fn steps(&self) -> &[String] {
        &self.steps
    }

    /// The session key used to store `step`'s data.
    pub fn session_key(&self, step: &str) -> String {
        format!("{WIZARD_KEY_PREFIX}:{}:{}", self.name, step)
    }

    /// Returns `true` if `step` has data persisted in the session.
    pub async fn is_step_complete(&self, step: &str) -> bool {
        let key = self.session_key(step);
        self.session.contains_key(&key).await
    }

    /// Returns the name of the first step that has *not* been completed, or
    /// `None` if all steps are complete.
    pub async fn first_incomplete_step(&self) -> Option<String> {
        for step in &self.steps {
            if !self.is_step_complete(step).await {
                return Some(step.clone());
            }
        }
        None
    }

    /// Guard for a step handler.
    ///
    /// If any earlier step is incomplete, returns `Err(redirect_url)` where
    /// the URL is `{base_path}/{first_incomplete_step}`.  When the current
    /// step is the first incomplete step (or all prior steps are complete),
    /// returns `Ok(())`.
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// if let Err(url) = wizard.guard_step("payment", "/checkout").await {
    ///     return Redirect::to(&url).into_response();
    /// }
    /// ```
    pub async fn guard_step(&self, current_step: &str, base_path: &str) -> Result<(), String> {
        let current_idx = match self.step_index(current_step) {
            Some(i) => i,
            None => return Ok(()), // unknown step — let the handler decide
        };
        for step in &self.steps[..current_idx] {
            if !self.is_step_complete(step).await {
                return Err(format!("{base_path}/{step}"));
            }
        }
        Ok(())
    }

    /// Persist `data` for `step` into the session as a JSON string.
    ///
    /// Overwrites any previously saved value for that step; later steps'
    /// data is **not** discarded, supporting back-navigation re-submission.
    ///
    /// # Errors
    ///
    /// Returns [`WizardError::Serialize`] when `data` cannot be serialized.
    pub async fn save_step<T: Serialize>(
        &self,
        step: &str,
        data: &T,
    ) -> Result<(), WizardError> {
        let json = serde_json::to_string(data)?;
        let key = self.session_key(step);
        self.session.insert(key, json).await;
        Ok(())
    }

    /// Load the persisted data for `step` from the session.
    ///
    /// Returns `None` when the step has not yet been completed or when
    /// deserialization fails (treat as not-started).
    pub async fn step_data<T: DeserializeOwned>(&self, step: &str) -> Option<T> {
        let key = self.session_key(step);
        let json = self.session.get(&key).await?;
        serde_json::from_str(&json).ok()
    }

    /// Remove all wizard-state keys for this wizard from the session.
    ///
    /// Call on `commit` (successful completion) or `cancel`.  Abandoned
    /// wizards expire with the session automatically — no separate TTL needed.
    pub async fn clear(&self) {
        for step in &self.steps {
            let key = self.session_key(step);
            self.session.remove(&key).await;
        }
    }

    /// Convenience: load data for all steps at once.
    ///
    /// Returns a `Vec<Option<serde_json::Value>>` (one entry per step in
    /// declaration order).  Use [`step_data`](Self::step_data) when you need
    /// a typed value for a specific step.
    pub async fn all_step_data_json(&self) -> Vec<Option<serde_json::Value>> {
        let mut out = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let key = self.session_key(step);
            let val = self
                .session
                .get(&key)
                .await
                .and_then(|j| serde_json::from_str(&j).ok());
            out.push(val);
        }
        out
    }
}

// ── Progress/stepper rendering ─────────────────────────────────────

/// The display state of a single step in the stepper UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// All required data was saved and the step is done.
    Completed,
    /// The step the user is currently on.
    Current,
    /// The step has not been reached yet.
    Upcoming,
}

/// A single step descriptor for the progress/stepper helper.
#[derive(Debug, Clone)]
pub struct WizardProgressStep {
    /// Step name (matches the name in [`WizardContext`]).
    pub name: String,
    /// Human-readable label (title-cased step name by default).
    pub label: String,
    /// Display state.
    pub state: StepState,
}

/// Compute the progress steps for rendering a stepper widget.
///
/// Each step's state is determined by checking the session, so this is
/// `async`.  Call [`wizard_progress`] (with the `maud` feature) to render
/// directly, or use the returned `Vec` to build a custom template.
pub async fn wizard_progress_steps(
    ctx: &WizardContext,
    current_step: &str,
) -> Vec<WizardProgressStep> {
    let current_idx = ctx.step_index(current_step);
    let mut result = Vec::with_capacity(ctx.steps.len());
    for (i, step) in ctx.steps.iter().enumerate() {
        let state = match current_idx {
            Some(cur) if i < cur => StepState::Completed,
            Some(cur) if i == cur => StepState::Current,
            _ => {
                if ctx.is_step_complete(step).await {
                    StepState::Completed
                } else {
                    StepState::Upcoming
                }
            }
        };
        result.push(WizardProgressStep {
            name: step.clone(),
            label: title_case(step),
            state,
        });
    }
    result
}

/// Render an accessible step-progress indicator as Maud [`Markup`].
///
/// Emits a `<nav>` with `role="list"` and per-step `<li>` items.  The
/// current step gets `aria-current="step"`.
///
/// ```html
/// <nav aria-label="Checkout progress">
///   <ol role="list" class="wizard-progress">
///     <li class="wizard-step wizard-step--completed">…Shipping…</li>
///     <li class="wizard-step wizard-step--current" aria-current="step">…Payment…</li>
///     <li class="wizard-step wizard-step--upcoming">…Review…</li>
///   </ol>
/// </nav>
/// ```
#[cfg(feature = "maud")]
pub async fn wizard_progress(ctx: &WizardContext, current_step: &str) -> Markup {
    let steps = wizard_progress_steps(ctx, current_step).await;
    let total = steps.len();
    let current_number = ctx.step_number(current_step).unwrap_or(1);

    html! {
        nav aria-label="Progress" {
            ol role="list" class="wizard-progress" {
                @for (i, step) in steps.iter().enumerate() {
                    @let step_num = i + 1;
                    @let class = match step.state {
                        StepState::Completed => "wizard-step wizard-step--completed",
                        StepState::Current   => "wizard-step wizard-step--current",
                        StepState::Upcoming  => "wizard-step wizard-step--upcoming",
                    };
                    @if step.state == StepState::Current {
                        li class=(class) aria-current="step" {
                            span class="wizard-step__number" { (step_num) }
                            span class="wizard-step__label" { (step.label) }
                        }
                    } @else {
                        li class=(class) {
                            span class="wizard-step__number" { (step_num) }
                            span class="wizard-step__label" { (step.label) }
                        }
                    }
                }
            }
            p class="wizard-progress__summary" aria-live="polite" {
                "Step " (current_number) " of " (total)
            }
        }
    }
}

/// Convert a `snake_case` or `kebab-case` step name into a Title Case label.
fn title_case(s: &str) -> String {
    s.split(|c| c == '_' || c == '-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    fn make_session() -> Session {
        Session::new_for_test("test-session-id".to_string(), HashMap::new())
    }

    fn make_wizard(session: Session) -> WizardContext {
        WizardContext::new(
            session,
            "checkout",
            vec!["shipping".into(), "payment".into(), "review".into()],
        )
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct ShippingData {
        address: String,
        city: String,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct PaymentData {
        card_last4: String,
    }

    // ── session key format ─────────────────────────────────────────

    #[test]
    fn session_key_uses_namespaced_format() {
        let wizard = make_wizard(make_session());
        assert_eq!(wizard.session_key("shipping"), "__autumn_wizard:checkout:shipping");
        assert_eq!(wizard.session_key("payment"), "__autumn_wizard:checkout:payment");
    }

    // ── step metadata ──────────────────────────────────────────────

    #[test]
    fn total_steps_returns_correct_count() {
        let wizard = make_wizard(make_session());
        assert_eq!(wizard.total_steps(), 3);
    }

    #[test]
    fn step_index_returns_zero_based_position() {
        let wizard = make_wizard(make_session());
        assert_eq!(wizard.step_index("shipping"), Some(0));
        assert_eq!(wizard.step_index("payment"), Some(1));
        assert_eq!(wizard.step_index("review"), Some(2));
        assert_eq!(wizard.step_index("missing"), None);
    }

    #[test]
    fn step_number_returns_one_based_display_number() {
        let wizard = make_wizard(make_session());
        assert_eq!(wizard.step_number("shipping"), Some(1));
        assert_eq!(wizard.step_number("payment"), Some(2));
        assert_eq!(wizard.step_number("review"), Some(3));
        assert_eq!(wizard.step_number("unknown"), None);
    }

    // ── save and load ──────────────────────────────────────────────

    #[tokio::test]
    async fn save_and_load_step_data_roundtrip() {
        let wizard = make_wizard(make_session());
        let data = ShippingData {
            address: "123 Main St".into(),
            city: "Springfield".into(),
        };
        wizard.save_step("shipping", &data).await.unwrap();
        let loaded: Option<ShippingData> = wizard.step_data("shipping").await;
        assert_eq!(loaded, Some(data));
    }

    #[tokio::test]
    async fn step_data_returns_none_for_unset_step() {
        let wizard = make_wizard(make_session());
        let loaded: Option<ShippingData> = wizard.step_data("shipping").await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn save_step_overwrites_without_discarding_other_steps() {
        let wizard = make_wizard(make_session());
        let shipping = ShippingData {
            address: "First".into(),
            city: "A".into(),
        };
        let payment = PaymentData { card_last4: "4242".into() };

        wizard.save_step("shipping", &shipping).await.unwrap();
        wizard.save_step("payment", &payment).await.unwrap();

        // Update shipping
        let shipping2 = ShippingData { address: "Second".into(), city: "B".into() };
        wizard.save_step("shipping", &shipping2).await.unwrap();

        // Payment is still there
        let loaded_payment: Option<PaymentData> = wizard.step_data("payment").await;
        assert_eq!(loaded_payment.as_ref().map(|p| p.card_last4.as_str()), Some("4242"));

        // Shipping is updated
        let loaded_shipping: Option<ShippingData> = wizard.step_data("shipping").await;
        assert_eq!(loaded_shipping.as_ref().map(|s| s.address.as_str()), Some("Second"));
    }

    // ── completion check ───────────────────────────────────────────

    #[tokio::test]
    async fn is_step_complete_reflects_session_state() {
        let wizard = make_wizard(make_session());
        assert!(!wizard.is_step_complete("shipping").await);

        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();

        assert!(wizard.is_step_complete("shipping").await);
    }

    #[tokio::test]
    async fn first_incomplete_step_returns_first_missing() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();

        assert_eq!(wizard.first_incomplete_step().await.as_deref(), Some("payment"));
    }

    #[tokio::test]
    async fn first_incomplete_step_returns_none_when_all_complete() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();
        wizard
            .save_step("payment", &PaymentData { card_last4: "4242".into() })
            .await
            .unwrap();
        wizard
            .save_step("review", &serde_json::json!({"confirmed": true}))
            .await
            .unwrap();

        assert!(wizard.first_incomplete_step().await.is_none());
    }

    // ── step guards ────────────────────────────────────────────────

    #[tokio::test]
    async fn guard_step_allows_first_step_unconditionally() {
        let wizard = make_wizard(make_session());
        // shipping is step 0 — no prior steps, always allowed
        assert!(wizard.guard_step("shipping", "/checkout").await.is_ok());
    }

    #[tokio::test]
    async fn guard_step_blocks_step_2_when_step_1_incomplete() {
        let wizard = make_wizard(make_session());
        // payment is step 1 — shipping (step 0) must be complete first
        let result = wizard.guard_step("payment", "/checkout").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "/checkout/shipping");
    }

    #[tokio::test]
    async fn guard_step_allows_step_2_when_step_1_complete() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();
        assert!(wizard.guard_step("payment", "/checkout").await.is_ok());
    }

    #[tokio::test]
    async fn guard_step_blocks_step_3_when_step_2_incomplete() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();
        // review is step 2 — payment (step 1) must be complete
        let result = wizard.guard_step("review", "/checkout").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "/checkout/payment");
    }

    // ── clear ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn clear_removes_all_wizard_keys() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();
        wizard
            .save_step("payment", &PaymentData { card_last4: "4242".into() })
            .await
            .unwrap();

        wizard.clear().await;

        assert!(!wizard.is_step_complete("shipping").await);
        assert!(!wizard.is_step_complete("payment").await);
    }

    #[tokio::test]
    async fn clear_does_not_remove_other_session_keys() {
        let session = make_session();
        session.insert("unrelated_key", "keep_me").await;
        let wizard = make_wizard(session.clone());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();

        wizard.clear().await;

        assert_eq!(session.get("unrelated_key").await.as_deref(), Some("keep_me"));
    }

    // ── all_step_data_json ─────────────────────────────────────────

    #[tokio::test]
    async fn all_step_data_json_returns_none_for_missing_steps() {
        let wizard = make_wizard(make_session());
        let all = wizard.all_step_data_json().await;
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|v| v.is_none()));
    }

    #[tokio::test]
    async fn all_step_data_json_returns_data_for_completed_steps() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();

        let all = wizard.all_step_data_json().await;
        assert!(all[0].is_some()); // shipping
        assert!(all[1].is_none()); // payment
        assert!(all[2].is_none()); // review
    }

    // ── title_case helper ──────────────────────────────────────────

    #[test]
    fn title_case_converts_snake_case() {
        assert_eq!(title_case("shipping"), "Shipping");
        assert_eq!(title_case("billing_address"), "Billing Address");
        assert_eq!(title_case("payment_method"), "Payment Method");
    }

    #[test]
    fn title_case_converts_kebab_case() {
        assert_eq!(title_case("payment-method"), "Payment Method");
    }

    // ── progress steps ─────────────────────────────────────────────

    #[tokio::test]
    async fn wizard_progress_steps_marks_current_and_upcoming() {
        let wizard = make_wizard(make_session());
        let steps = wizard_progress_steps(&wizard, "shipping").await;
        assert_eq!(steps[0].state, StepState::Current);
        assert_eq!(steps[1].state, StepState::Upcoming);
        assert_eq!(steps[2].state, StepState::Upcoming);
    }

    #[tokio::test]
    async fn wizard_progress_steps_marks_prior_steps_completed() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();

        let steps = wizard_progress_steps(&wizard, "payment").await;
        assert_eq!(steps[0].state, StepState::Completed);
        assert_eq!(steps[1].state, StepState::Current);
        assert_eq!(steps[2].state, StepState::Upcoming);
    }

    #[tokio::test]
    async fn wizard_progress_steps_labels_use_title_case() {
        let wizard = make_wizard(make_session());
        let steps = wizard_progress_steps(&wizard, "shipping").await;
        assert_eq!(steps[0].label, "Shipping");
        assert_eq!(steps[1].label, "Payment");
        assert_eq!(steps[2].label, "Review");
    }

    // ── maud rendering ─────────────────────────────────────────────

    #[cfg(feature = "maud")]
    #[tokio::test]
    async fn wizard_progress_renders_aria_current_on_current_step() {
        let wizard = make_wizard(make_session());
        let markup = wizard_progress(&wizard, "shipping").await;
        let html = markup.into_string();
        assert!(html.contains("aria-current=\"step\""));
    }

    #[cfg(feature = "maud")]
    #[tokio::test]
    async fn wizard_progress_renders_step_count_summary() {
        let wizard = make_wizard(make_session());
        let markup = wizard_progress(&wizard, "payment").await;
        let html = markup.into_string();
        assert!(html.contains("Step 2 of 3"));
    }

    #[cfg(feature = "maud")]
    #[tokio::test]
    async fn wizard_progress_renders_all_step_labels() {
        let wizard = make_wizard(make_session());
        let markup = wizard_progress(&wizard, "shipping").await;
        let html = markup.into_string();
        assert!(html.contains("Shipping"));
        assert!(html.contains("Payment"));
        assert!(html.contains("Review"));
    }

    #[cfg(feature = "maud")]
    #[tokio::test]
    async fn wizard_progress_applies_correct_css_classes() {
        let wizard = make_wizard(make_session());
        wizard
            .save_step("shipping", &ShippingData { address: "A".into(), city: "B".into() })
            .await
            .unwrap();
        let markup = wizard_progress(&wizard, "payment").await;
        let html = markup.into_string();
        assert!(html.contains("wizard-step--completed"));
        assert!(html.contains("wizard-step--current"));
        assert!(html.contains("wizard-step--upcoming"));
    }
}
