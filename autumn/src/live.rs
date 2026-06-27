//! Declarative live-broadcast trait for repository auto-broadcasting.
//!
//! Implement [`LiveFragment`] on any `#[model]` type to opt in to
//! `hx-swap-oob` broadcasts whenever the repository mutates that model.
//! Then add `broadcasts = "topic"` to your `#[repository]` attribute and
//! the generated `save`/`update`/`delete_by_id` methods will automatically
//! publish the rendered HTML fragment to the named channel.
//!
//! # Example
//!
//! ```rust,no_run
//! use autumn_web::live::LiveFragment;
//! use maud::{Markup, html};
//!
//! // Given a model:
//! // pub struct Task { pub id: i64, pub title: String }
//!
//! # struct Task { pub id: i64, pub title: String }
//! impl LiveFragment for Task {
//!     fn dom_id_for(id: i64) -> String {
//!         format!("task-{id}")
//!     }
//!
//!     fn dom_id(&self) -> String {
//!         Self::dom_id_for(self.id)
//!     }
//!
//!     fn render_fragment(&self) -> Markup {
//!         html! {
//!             li id=(self.dom_id()) { (self.title) }
//!         }
//!     }
//! }
//! // Then declare:
//! // #[autumn_web::repository(Task, broadcasts = "tasks")]
//! // pub trait TaskRepository {}
//! ```
//!
//! # Swap strategies
//!
//! | Mutation | Default swap |
//! |----------|-------------|
//! | `save`   | `insert_swap()` — default `OobSwap::True` (replace element) |
//! | `update` | `OobSwap::True` (replace element by matching id) |
//! | `delete` | `OobSwap::Delete` (remove element from DOM) |
//!
//! Override `insert_swap()` to use a different strategy for new records.
//! For example, to append new items to a list container:
//!
//! ```rust,no_run
//! # use autumn_web::htmx::OobSwap;
//! # struct Task;
//! # impl autumn_web::live::LiveFragment for Task {
//! #   fn dom_id_for(_: i64) -> String { String::new() }
//! #   fn dom_id(&self) -> String { String::new() }
//! #   fn render_fragment(&self) -> maud::Markup { maud::html!{} }
//!     fn insert_swap() -> OobSwap {
//!         OobSwap::BeforeEnd
//!     }
//! # }
//! ```
//!
//! # Note on `commit_hooks`
//!
//! Broadcasts fire inline after the mutation in the same async task. They are
//! **not** wired through the durable `after_*_commit` queue, which has no
//! channel handle. If the channel has no active subscribers the publish
//! silently succeeds with zero receivers.

/// Trait that connects a model to its live-broadcast HTML fragment.
///
/// Implement this on your model type and declare `broadcasts = "topic"` on the
/// `#[repository]` attribute to enable automatic OOB broadcasts after each
/// mutation.
///
/// Requires the `ws`, `maud`, and `htmx` features of `autumn-web`.
#[cfg(all(feature = "htmx", feature = "maud"))]
pub trait LiveFragment {
    /// Compute the DOM id for a primary key value.
    ///
    /// Used for delete broadcasts where only the `id` is available (the
    /// record has already been removed from the database).
    fn dom_id_for(id: i64) -> String;

    /// Compute the DOM id for this model instance.
    ///
    /// The root element of [`render_fragment`](Self::render_fragment) **must**
    /// carry `id = self.dom_id()` so htmx can match it for OOB swaps.
    fn dom_id(&self) -> String;

    /// Render the single-item HTML fragment for this record.
    ///
    /// The root element must have `id = self.dom_id()`.
    fn render_fragment(&self) -> maud::Markup;

    /// Htmx swap strategy to use when a new record is inserted.
    ///
    /// Defaults to [`OobSwap::True`] (replace an element with the matching id).
    /// Override to use `OobSwap::BeforeEnd` (append to a list container) or
    /// any other strategy.
    fn insert_swap() -> crate::htmx::OobSwap {
        crate::htmx::OobSwap::True
    }
}

#[cfg(test)]
#[cfg(all(feature = "htmx", feature = "maud"))]
mod tests {
    use super::*;

    struct Thing {
        id: i64,
        label: String,
    }

    impl LiveFragment for Thing {
        fn dom_id_for(id: i64) -> String {
            format!("thing-{id}")
        }

        fn dom_id(&self) -> String {
            Self::dom_id_for(self.id)
        }

        fn render_fragment(&self) -> maud::Markup {
            maud::html! {
                li id=(self.dom_id()) { (self.label) }
            }
        }
    }

    #[test]
    fn dom_id_for_formats_correctly() {
        assert_eq!(Thing::dom_id_for(42), "thing-42");
    }

    #[test]
    fn dom_id_matches_dom_id_for() {
        let t = Thing {
            id: 7,
            label: "x".into(),
        };
        assert_eq!(t.dom_id(), Thing::dom_id_for(7));
    }

    #[test]
    fn render_fragment_contains_id() {
        let t = Thing {
            id: 5,
            label: "hello".into(),
        };
        let html = t.render_fragment().into_string();
        assert!(
            html.contains("thing-5"),
            "fragment must embed dom_id: {html}"
        );
        assert!(
            html.contains("hello"),
            "fragment must embed content: {html}"
        );
    }

    #[test]
    fn insert_swap_defaults_to_true() {
        assert!(matches!(Thing::insert_swap(), crate::htmx::OobSwap::True));
    }
}
