//! Flash messages for Autumn applications.
//!
//! Provides a [`Flash`] extractor that allows storing and retrieving
//! temporary messages across HTTP redirects, backed by the user's
//! [`crate::session::Session`].
//!
//! # Examples
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use axum::response::{IntoResponse, Redirect};
//!
//! #[post("/items")]
//! async fn create_item(flash: Flash) -> impl IntoResponse {
//!     // ... create item ...
//!     flash.success("Item created successfully!").await;
//!     Redirect::to("/items")
//! }
//!
//! #[get("/items")]
//! async fn list_items(flash: Flash) -> Markup {
//!     let messages = flash.consume().await;
//!     html! {
//!         // ... render messages ...
//!         @for msg in messages {
//!             div class=(msg.level.as_str()) { (msg.message) }
//!         }
//!     }
//! }
//! ```

use axum::extract::FromRequestParts;
use http::request::Parts;
use serde::{Deserialize, Serialize};

use crate::session::Session;

const FLASH_SESSION_KEY: &str = "__autumn_flash";

/// The severity level of a flash message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FlashLevel {
    /// Success messages (e.g., "Item created").
    Success,
    /// Informational messages (e.g., "Welcome back").
    Info,
    /// Warning messages (e.g., "Your trial ends soon").
    Warning,
    /// Error messages (e.g., "Invalid password").
    Error,
}

impl FlashLevel {
    /// Returns the level as a lowercase string (useful for CSS classes).
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A single flash message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashMessage {
    /// The severity level.
    pub level: FlashLevel,
    /// The text message.
    pub message: String,
}

/// Extractor for adding and consuming flash messages.
///
/// Backed by the session. Messages are stored as a JSON array
/// under the key `__autumn_flash`.
#[derive(Debug, Clone)]
pub struct Flash {
    session: Session,
}

impl Flash {
    /// Create a new `Flash` instance wrapping the given `Session`.
    #[must_use]
    pub const fn new(session: Session) -> Self {
        Self { session }
    }

    /// Add a new message to the flash queue.
    pub async fn push(&self, level: FlashLevel, message: impl Into<String>) {
        let mut messages = self.peek().await;
        messages.push(FlashMessage {
            level,
            message: message.into(),
        });

        if let Ok(json) = serde_json::to_string(&messages) {
            self.session.insert(FLASH_SESSION_KEY, json).await;
        }
    }

    /// Add a success message.
    pub async fn success(&self, message: impl Into<String>) {
        self.push(FlashLevel::Success, message).await;
    }

    /// Add an informational message.
    pub async fn info(&self, message: impl Into<String>) {
        self.push(FlashLevel::Info, message).await;
    }

    /// Add a warning message.
    pub async fn warning(&self, message: impl Into<String>) {
        self.push(FlashLevel::Warning, message).await;
    }

    /// Add an error message.
    pub async fn error(&self, message: impl Into<String>) {
        self.push(FlashLevel::Error, message).await;
    }

    /// Read all pending flash messages without removing them.
    pub async fn peek(&self) -> Vec<FlashMessage> {
        self.session
            .get(FLASH_SESSION_KEY)
            .await
            .map_or_else(Vec::new, |json| {
                serde_json::from_str(&json).unwrap_or_default()
            })
    }

    /// Read all pending flash messages and remove them from the session.
    pub async fn consume(&self) -> Vec<FlashMessage> {
        let messages = self.peek().await;
        if !messages.is_empty() {
            self.session.remove(FLASH_SESSION_KEY).await;
        }
        messages
    }

    /// Injects pending flash messages into an HTMX response as `HX-Trigger` events.
    ///
    /// Consumes the messages from the session and sets the `HX-Trigger` header
    /// with a JSON payload representing the messages. This allows the frontend
    /// to display flash messages without a full page reload.
    #[cfg(feature = "htmx")]
    pub async fn inject_hx_trigger<T: axum::response::IntoResponse>(
        &self,
        response: T,
    ) -> axum::response::Response {
        let messages = self.consume().await;
        let mut res = response.into_response();
        if !messages.is_empty() {
            let payload = serde_json::json!({
                "flash": messages
            });
            if let Ok(v) = http::header::HeaderValue::from_str(&payload.to_string()) {
                res.headers_mut()
                    .insert(http::header::HeaderName::from_static("hx-trigger"), v);
            }
        }
        res
    }
}

#[cfg(feature = "maud")]
impl Flash {
    /// Consume all pending flash messages and render them as HTML.
    ///
    /// This is the one-line helper for a base layout: drop `(flash.render().await)`
    /// into your template and every pending notice is rendered and cleared in a
    /// single call — no manual `consume()` + loop required.
    ///
    /// The output is wrapped in a stable `<div id="flash">` container that is
    /// *always* emitted, even when there are no messages, so it can act as the
    /// target for htmx out-of-band swaps on later requests.
    ///
    /// ```rust,no_run
    /// use autumn_web::prelude::*;
    ///
    /// #[get("/items")]
    /// async fn list_items(flash: Flash) -> Markup {
    ///     html! {
    ///         (flash.render().await)
    ///         h1 { "Items" }
    ///     }
    /// }
    /// ```
    pub async fn render(&self) -> maud::Markup {
        self.render_inner(false).await
    }

    /// Like [`render`](Self::render), but marks the container for an htmx
    /// out-of-band swap (`hx-swap-oob="true"`).
    ///
    /// Include `(flash.render_oob().await)` anywhere in an htmx partial response
    /// and the flash container in the already-rendered page is replaced in place,
    /// so notices appear on htmx-driven swaps — not just full-page loads. For the
    /// header-based alternative see [`inject_hx_trigger`](Self::inject_hx_trigger).
    pub async fn render_oob(&self) -> maud::Markup {
        self.render_inner(true).await
    }

    async fn render_inner(&self, oob: bool) -> maud::Markup {
        let messages = self.consume().await;
        maud::html! {
            div id="flash" class="flash-messages" role="status" aria-live="polite"
                hx-swap-oob=[oob.then_some("true")] {
                @for msg in &messages {
                    div class={ "flash flash-" (msg.level.as_str()) } role="alert"
                        style=(level_style(msg.level)) {
                        (msg.message)
                    }
                }
            }
        }
    }
}

/// Minimal inline styling per level so a generated app shows a *visible* notice
/// with zero added CSS. Apps that want full control can target the
/// `.flash` / `.flash-<level>` classes and ignore these defaults.
#[cfg(feature = "maud")]
const fn level_style(level: FlashLevel) -> &'static str {
    // Shared box layout plus a per-level color palette, inlined per variant so
    // this stays a `const fn` returning a single `&'static str`.
    match level {
        FlashLevel::Success => {
            "padding:0.75rem 1rem;border-radius:0.375rem;margin-bottom:0.5rem;border:1px solid;\
             background:#ecfdf5;color:#065f46;border-color:#6ee7b7;"
        }
        FlashLevel::Info => {
            "padding:0.75rem 1rem;border-radius:0.375rem;margin-bottom:0.5rem;border:1px solid;\
             background:#eff6ff;color:#1e3a8a;border-color:#93c5fd;"
        }
        FlashLevel::Warning => {
            "padding:0.75rem 1rem;border-radius:0.375rem;margin-bottom:0.5rem;border:1px solid;\
             background:#fffbeb;color:#92400e;border-color:#fcd34d;"
        }
        FlashLevel::Error => {
            "padding:0.75rem 1rem;border-radius:0.375rem;margin-bottom:0.5rem;border:1px solid;\
             background:#fef2f2;color:#991b1b;border-color:#fca5a5;"
        }
    }
}

impl<S> FromRequestParts<S> for Flash
where
    S: Send + Sync,
{
    type Rejection = <Session as FromRequestParts<S>>::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state).await?;
        Ok(Self::new(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn flash_push_and_consume() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session.clone());

        flash.success("Saved!").await;
        flash.error("Failed!").await;

        let messages = flash.peek().await;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].level, FlashLevel::Success);
        assert_eq!(messages[0].message, "Saved!");
        assert_eq!(messages[1].level, FlashLevel::Error);
        assert_eq!(messages[1].message, "Failed!");

        // Still there after peek
        assert_eq!(flash.peek().await.len(), 2);

        // Consume removes them
        let consumed = flash.consume().await;
        assert_eq!(consumed.len(), 2);
        assert_eq!(flash.peek().await.len(), 0);
    }

    #[tokio::test]
    async fn flash_level_as_str() {
        assert_eq!(FlashLevel::Success.as_str(), "success");
        assert_eq!(FlashLevel::Info.as_str(), "info");
        assert_eq!(FlashLevel::Warning.as_str(), "warning");
        assert_eq!(FlashLevel::Error.as_str(), "error");
    }

    #[tokio::test]
    async fn should_not_remove_key_when_consuming_empty_flash() -> Result<(), String> {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        // Insert a dummy key to verify the session remains untouched and "dirty" flag logic
        session.insert("dummy", "val").await;

        let flash = Flash::new(session.clone());
        let messages = flash.consume().await;

        // No messages were present
        assert_eq!(messages.len(), 0);

        // "dummy" key is still there
        assert_eq!(
            session.get("dummy").await.ok_or("missing key dummy")?,
            "val"
        );
        // Flash key shouldn't be added or touched
        assert!(!session.contains_key(FLASH_SESSION_KEY).await);
        Ok(())
    }

    #[tokio::test]
    async fn should_handle_invalid_json_gracefully() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        // Insert broken JSON manually
        session
            .insert(FLASH_SESSION_KEY, "{ invalid_json: true")
            .await;

        let flash = Flash::new(session);
        let messages = flash.peek().await;

        // It should gracefully fall back to an empty vector rather than panicking
        assert_eq!(messages.len(), 0);
    }

    #[tokio::test]
    async fn should_support_all_convenience_methods() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session);

        flash.success("Success msg").await;
        flash.info("Info msg").await;
        flash.warning("Warning msg").await;
        flash.error("Error msg").await;

        let messages = flash.peek().await;
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].level, FlashLevel::Success);
        assert_eq!(messages[0].message, "Success msg");
        assert_eq!(messages[1].level, FlashLevel::Info);
        assert_eq!(messages[1].message, "Info msg");
        assert_eq!(messages[2].level, FlashLevel::Warning);
        assert_eq!(messages[2].message, "Warning msg");
        assert_eq!(messages[3].level, FlashLevel::Error);
        assert_eq!(messages[3].message, "Error msg");
    }

    #[tokio::test]
    #[cfg(feature = "maud")]
    async fn render_emits_messages_and_clears_them() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session.clone());

        flash.success("Saved!").await;
        flash.error("Oops").await;

        let markup = flash.render().await.into_string();
        // Stable container that doubles as the htmx OOB target.
        assert!(markup.contains("id=\"flash\""), "missing container: {markup}");
        assert!(markup.contains("aria-live=\"polite\""));
        // Per-message level classes and text.
        assert!(markup.contains("flash flash-success"));
        assert!(markup.contains("Saved!"));
        assert!(markup.contains("flash flash-error"));
        assert!(markup.contains("Oops"));
        // Inline styles guarantee visibility with zero CSS plumbing.
        assert!(markup.contains("style="), "expected inline styling: {markup}");
        // A plain full-page render is not an out-of-band swap.
        assert!(!markup.contains("hx-swap-oob"));

        // render() consumes — the next render is empty.
        assert_eq!(flash.peek().await.len(), 0);
    }

    #[tokio::test]
    #[cfg(feature = "maud")]
    async fn render_emits_container_even_when_empty() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session);

        // No messages pushed — the container must still render so htmx OOB
        // swaps have a stable target on subsequent requests.
        let markup = flash.render().await.into_string();
        assert!(markup.contains("id=\"flash\""), "missing container: {markup}");
        assert!(!markup.contains("flash flash-"));
    }

    #[tokio::test]
    #[cfg(feature = "maud")]
    async fn render_oob_marks_container_for_out_of_band_swap() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session.clone());

        flash.info("Updated").await;

        let markup = flash.render_oob().await.into_string();
        assert!(markup.contains("id=\"flash\""));
        assert!(markup.contains("hx-swap-oob=\"true\""), "missing OOB attr: {markup}");
        assert!(markup.contains("flash flash-info"));
        assert!(markup.contains("Updated"));

        // Like render(), render_oob() consumes.
        assert_eq!(flash.peek().await.len(), 0);
    }

    #[tokio::test]
    #[cfg(feature = "htmx")]
    async fn should_inject_hx_trigger() {
        let session = Session::new_for_test("test_id".to_string(), HashMap::new());
        let flash = Flash::new(session.clone());

        flash.success("Item saved").await;

        let response = flash.inject_hx_trigger("OK").await;
        let header = response.headers().get("hx-trigger");
        assert!(header.is_some());

        let json_str = header.unwrap().to_str().unwrap();
        let payload: serde_json::Value = serde_json::from_str(json_str).unwrap();

        assert_eq!(payload["flash"][0]["level"], "success");
        assert_eq!(payload["flash"][0]["message"], "Item saved");
    }
}
