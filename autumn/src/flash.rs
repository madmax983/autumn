//! Flash extractor for temporary session-based messages.
//!
//! Provides a `Flash` extractor which is backed by `Session`.
//! Allows pushing a message for the next request, and retrieving
//! it exactly once on the following request.

use crate::error::AutumnError;
use crate::session::Session;
use crate::state::AppState;
use axum::extract::FromRequestParts;

/// The key used to store flash messages in the session.
const FLASH_KEY: &str = "_flash";

/// Temporary session-based message.
///
/// Can be used as an extractor in Axum handlers, which removes
/// the message from the session upon extraction.
///
/// Allows pushing new messages to the session to be read on
/// the next request.
pub struct Flash {
    message: Option<String>,
    session: Session,
}

impl Flash {
    /// Retrieve the flash message, if any.
    /// Note: The message is already removed from the session by the extractor.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Set a new flash message for the next request.
    pub async fn push(&self, message: impl Into<String>) {
        self.session.insert(FLASH_KEY, message.into()).await;
    }
}

impl FromRequestParts<AppState> for Flash {
    type Rejection = AutumnError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state).await?;
        let message = session.remove(FLASH_KEY).await;

        Ok(Self { message, session })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use std::collections::HashMap;

    #[tokio::test]
    async fn flash_extractor_reads_and_removes_message() {
        // Create a session with a flash message
        let mut data = HashMap::new();
        data.insert(FLASH_KEY.into(), "hello world".into());
        let session = Session::new_for_test("test".into(), data);

        // Put the session in request extensions
        let mut req = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(session.clone());
        let (mut parts, ()) = req.into_parts();

        let state = AppState::for_test();

        // Extract Flash
        let flash = Flash::from_request_parts(&mut parts, &state).await.unwrap();

        // Verify the message was extracted
        assert_eq!(flash.message(), Some("hello world"));

        // Verify it was removed from the session
        assert!(session.get(FLASH_KEY).await.is_none());
    }

    #[tokio::test]
    async fn flash_push_sets_message() {
        // Create an empty session
        let session = Session::new_for_test("test".into(), HashMap::new());

        // Put the session in request extensions
        let mut req = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(session.clone());
        let (mut parts, ()) = req.into_parts();

        let state = AppState::for_test();

        // Extract Flash
        let flash = Flash::from_request_parts(&mut parts, &state).await.unwrap();

        // Verify no message initially
        assert_eq!(flash.message(), None);

        // Push a new message
        flash.push("new message").await;

        // Verify it was added to the session
        assert_eq!(
            session.get(FLASH_KEY).await,
            Some("new message".to_string())
        );
    }
}
