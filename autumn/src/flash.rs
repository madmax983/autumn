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
