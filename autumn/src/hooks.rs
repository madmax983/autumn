//! Mutation hook types for repository lifecycle callbacks.
//!
//! This module provides foundational types used by generated repository code
//! to support before/after mutation hooks (create, update, delete).

use serde::{Deserialize, Serialize};

// ── Mutation operation & context ─────────────────────────────────────

/// The kind of mutation being performed on a repository record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOp {
    /// A new record is being created.
    Create,
    /// An existing record is being updated.
    Update,
    /// An existing record is being deleted.
    Delete,
}

impl MutationOp {
    /// Returns the operation name as a static string slice.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl std::fmt::Display for MutationOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Context available to mutation hooks.
///
/// Carries actor identity, request metadata, and timestamps so that
/// hook implementations can perform auditing, validation, or enrichment.
#[derive(Debug, Clone)]
pub struct MutationContext {
    /// The mutation operation type.
    pub op: MutationOp,
    /// Actor identity (user ID or service name). `None` for anonymous.
    pub actor: Option<String>,
    /// Correlation / request ID for tracing.
    pub request_id: Option<String>,
    /// Timestamp of the mutation.
    pub now: chrono::DateTime<chrono::Utc>,
}

impl MutationContext {
    /// Create a new context for the given operation.
    ///
    /// Auto-populates `now` with `Utc::now()` and `request_id` with a
    /// freshly generated UUID v4.
    #[must_use]
    pub fn new(op: MutationOp) -> Self {
        Self {
            op,
            actor: None,
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            now: chrono::Utc::now(),
        }
    }
}

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

/// Per-field before/after diff accessor for mutation hooks.
///
/// `FieldDiff<T>` holds the previous and proposed values for a single field,
/// allowing hook authors to inspect what changed and optionally override the
/// new value via [`set`](FieldDiff::set).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff<T> {
    before: T,
    after: T,
}

impl<T: PartialEq> FieldDiff<T> {
    /// Create a new diff from before and after values.
    #[must_use]
    pub const fn new(before: T, after: T) -> Self {
        Self { before, after }
    }

    /// Reference to the value before the mutation.
    #[must_use]
    pub const fn before(&self) -> &T {
        &self.before
    }

    /// Reference to the (possibly overridden) value after the mutation.
    #[must_use]
    pub const fn after(&self) -> &T {
        &self.after
    }

    /// Returns `true` if the field value changed.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.before != self.after
    }

    /// Returns `true` if the field value did not change.
    #[must_use]
    pub fn unchanged(&self) -> bool {
        self.before == self.after
    }

    /// Returns `true` if the field changed **and** the new value equals `value`.
    #[must_use]
    pub fn changed_to(&self, value: &T) -> bool {
        self.changed() && self.after == *value
    }

    /// Returns `true` if the field changed **and** the old value equals `value`.
    #[must_use]
    pub fn changed_from(&self, value: &T) -> bool {
        self.changed() && self.before == *value
    }

    /// Override the after value. Does not affect `before`.
    pub fn set(&mut self, value: T) {
        self.after = value;
    }
}

impl<T: PartialEq> FieldDiff<Option<T>> {
    /// Returns `true` if the field went from `None` to `Some`.
    #[must_use]
    pub const fn was_set(&self) -> bool {
        self.before.is_none() && self.after.is_some()
    }

    /// Returns `true` if the field went from `Some` to `None`.
    #[must_use]
    pub const fn was_cleared(&self) -> bool {
        self.before.is_some() && self.after.is_none()
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

    // ── FieldDiff tests ──────────────────────────────────────────

    #[test]
    fn field_diff_unchanged() {
        let diff = FieldDiff::new(1, 1);
        assert!(diff.unchanged());
        assert!(!diff.changed());
    }

    #[test]
    fn field_diff_changed() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed());
    }

    #[test]
    fn field_diff_changed_to() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed_to(&2));
    }

    #[test]
    fn field_diff_changed_from() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed_from(&1));
    }

    #[test]
    fn field_diff_set_updates_after() {
        let mut diff = FieldDiff::new(1, 1);
        assert!(diff.unchanged());
        diff.set(5);
        assert!(diff.changed());
        assert_eq!(diff.after(), &5);
        assert_eq!(diff.before(), &1);
    }

    #[test]
    fn field_diff_option_was_set() {
        let diff = FieldDiff::new(None, Some(42));
        assert!(diff.was_set());
    }

    #[test]
    fn field_diff_option_was_cleared() {
        let diff = FieldDiff::new(Some(42), None);
        assert!(diff.was_cleared());
    }

    // ── MutationOp tests ────────────────────────────────────────────

    #[test]
    fn mutation_op_as_str() {
        assert_eq!(MutationOp::Create.as_str(), "create");
        assert_eq!(MutationOp::Update.as_str(), "update");
        assert_eq!(MutationOp::Delete.as_str(), "delete");
    }

    #[test]
    fn mutation_op_display() {
        assert_eq!(format!("{}", MutationOp::Create), "create");
    }

    // ── MutationContext tests ───────────────────────────────────────

    #[test]
    fn mutation_context_auto_populates() {
        let ctx = MutationContext::new(MutationOp::Create);
        assert!(ctx.actor.is_none());
        assert!(ctx.request_id.is_some());
        // UUID v4 format: 8-4-4-4-12 = 36 chars
        assert_eq!(ctx.request_id.as_ref().unwrap().len(), 36);
        assert!(matches!(ctx.op, MutationOp::Create));
    }

    #[test]
    fn mutation_context_with_actor() {
        let mut ctx = MutationContext::new(MutationOp::Update);
        ctx.actor = Some("user-123".into());
        assert_eq!(ctx.actor.as_deref(), Some("user-123"));
    }
}
