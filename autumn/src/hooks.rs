//! Mutation hook types for repository lifecycle callbacks.
//!
//! This module provides foundational types used by generated repository code
//! to support before/after mutation hooks (create, update, delete).

use serde::{Deserialize, Serialize};

/// Tri-state sparse update value.
///
/// `Patch<T>` distinguishes between "field not mentioned" ([`Unchanged`](Patch::Unchanged)),
/// "field explicitly set" ([`Set`](Patch::Set)), and "field explicitly cleared"
/// ([`Clear`](Patch::Clear), mapping to SQL `NULL`).
///
/// This is the building block for partial-update (PATCH) payloads where
/// omitting a field means "leave it alone" rather than "set it to its
/// default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Patch<T> {
    /// The field was not included in the update payload.
    #[default]
    Unchanged,
    /// The field was explicitly set to a new value.
    Set(T),
    /// The field was explicitly cleared (maps to SQL `NULL`).
    Clear,
}

impl<T> Patch<T> {
    /// Returns `true` if the field was not included in the update.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// Returns `true` if the field was explicitly set to a new value.
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::Set(_))
    }

    /// Returns `true` if the field was explicitly cleared.
    #[must_use]
    pub const fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Returns a reference to the inner value if [`Set`](Patch::Set), or `None`.
    #[must_use]
    pub const fn as_set(&self) -> Option<&T> {
        match self {
            Self::Set(v) => Some(v),
            _ => None,
        }
    }

    /// Converts into a nested `Option`:
    ///
    /// - `Set(v)` -> `Some(Some(v))`
    /// - `Clear` -> `Some(None)`
    /// - `Unchanged` -> `None`
    #[must_use]
    pub fn into_option(self) -> Option<Option<T>> {
        match self {
            Self::Set(v) => Some(Some(v)),
            Self::Clear => Some(None),
            Self::Unchanged => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Patch tests ──────────────────────────────────────────────

    #[test]
    fn patch_unchanged_is_default() {
        let p: Patch<String> = Patch::default();
        assert!(p.is_unchanged());
    }

    #[test]
    fn patch_set_holds_value() {
        let p = Patch::Set("hello");
        assert!(p.is_set());
        assert_eq!(p.as_set(), Some(&"hello"));
    }

    #[test]
    fn patch_clear_is_clear() {
        let p: Patch<i32> = Patch::Clear;
        assert!(p.is_clear());
        assert!(!p.is_set());
        assert!(!p.is_unchanged());
    }

    #[test]
    fn patch_into_option_set() {
        assert_eq!(Patch::Set(42).into_option(), Some(Some(42)));
    }

    #[test]
    fn patch_into_option_clear() {
        assert_eq!(Patch::<i32>::Clear.into_option(), Some(None));
    }

    #[test]
    fn patch_into_option_unchanged() {
        assert_eq!(Patch::<i32>::Unchanged.into_option(), None);
    }
}
