//! Active search and autocomplete form primitives with htmx integration.
//!
//! These helpers generate server-rendered HTML with embedded htmx attributes
//! so Autumn applications can add live search and autocomplete with zero
//! custom JavaScript.
//!
//! # Choosing the right primitive
//!
//! | Situation | Use |
//! |-----------|-----|
//! | Keyword search over a rendered list | `active_search` / `active_search_input` |
//! | Select a single related record and store its ID | `autocomplete_input` |
//! | Plain `GET` form is sufficient | `axum::extract::Query` |
//! | You need unusual htmx wiring | Hand-write `hx-*` attributes |
//!
//! # Integration with the repository full-text search feature
//!
//! The widgets wire up the client side; your handler owns the Diesel query.
//! If your repository has `#[repository(..., searchable)]`, the generated
//! `repo.search(q)` method works directly with these widgets:
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::widgets::{ActiveSearchConfig, active_search};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct SearchQuery { q: String }
//!
//! #[get("/posts")]
//! async fn index() -> Markup {
//!     let config = ActiveSearchConfig::new("/posts/search", "#post-results");
//!     html! {
//!         (active_search("post-search", "Search posts", &config))
//!     }
//! }
//!
//! #[get("/posts/search")]
//! async fn search(
//!     Query(params): Query<SearchQuery>,
//!     repo: PgPostRepository,
//! ) -> AutumnResult<Markup> {
//!     if params.q.trim().is_empty() {
//!         return Ok(active_search_empty_state("Enter a search term"));
//!     }
//!     let results = repo.search(&params.q).await?;
//!     Ok(html! {
//!         @if results.is_empty() {
//!             (active_search_empty_state("No results found"))
//!         } @else {
//!             @for post in &results { li { (post.title) } }
//!         }
//!     })
//! }
//! ```
//!
//! # No-JavaScript fallback
//!
//! `active_search` and `autocomplete_input` include a `<noscript>` block
//! with a plain HTML form or select that works without JavaScript. Your handler
//! already returns an HTML fragment — the only addition for a full no-JS page
//! is wrapping the response in your layout template.

/// HTTP method for an [`ActiveSearchConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchMethod {
    /// `GET` request — the default. Safe, idempotent, bookmarkable.
    #[default]
    Get,
    /// `POST` request — opt-in for handlers that need a request body.
    Post,
}

/// Configuration for an [`active_search_input`] widget.
///
/// Build with [`ActiveSearchConfig::new`] and chain builder methods for
/// optional overrides.
///
/// # Example
///
/// ```rust,ignore
/// use autumn_web::widgets::{ActiveSearchConfig, active_search};
///
/// let config = ActiveSearchConfig::new("/users/search", "#user-results")
///     .debounce(500)
///     .min_length(2)
///     .placeholder("Search users…");
///
/// let widget = active_search("user-search", "Search users", &config);
/// ```
#[derive(Debug, Clone)]
pub struct ActiveSearchConfig<'a> {
    /// URL of the server-side search handler.
    pub action: &'a str,
    /// HTTP method (default: [`SearchMethod::Get`]).
    pub method: SearchMethod,
    /// CSS selector for the element that receives rendered results.
    pub target: &'a str,
    /// CSS selector for an element shown while the request is in flight.
    pub indicator: Option<&'a str>,
    /// Debounce delay in milliseconds (default: `300`).
    pub debounce_ms: u32,
    /// Minimum character count before triggering a search (default: `1`).
    pub min_length: u32,
    /// Query parameter name sent to the handler (default: `"q"`).
    pub param_name: &'a str,
    /// Whether to fire the search immediately on page load (default: `false`).
    pub initial_load: bool,
    /// Optional placeholder text for the search input.
    pub placeholder: Option<&'a str>,
}

impl<'a> ActiveSearchConfig<'a> {
    /// Create a new active search configuration with sensible defaults.
    ///
    /// - `action` — URL of the search handler
    /// - `target` — CSS selector for the results container, e.g. `"#search-results"`
    #[must_use]
    pub const fn new(action: &'a str, target: &'a str) -> Self {
        Self {
            action,
            method: SearchMethod::Get,
            target,
            indicator: None,
            debounce_ms: 300,
            min_length: 1,
            param_name: "q",
            initial_load: false,
            placeholder: None,
        }
    }

    /// Use `POST` instead of the default `GET` for the search request.
    #[must_use]
    pub const fn post(mut self) -> Self {
        self.method = SearchMethod::Post;
        self
    }

    /// Set the debounce delay in milliseconds (default: `300`).
    #[must_use]
    pub const fn debounce(mut self, ms: u32) -> Self {
        self.debounce_ms = ms;
        self
    }

    /// Set the minimum query length before a search is triggered (default: `1`).
    #[must_use]
    pub const fn min_length(mut self, n: u32) -> Self {
        self.min_length = n;
        self
    }

    /// Set a CSS selector for the htmx loading indicator element.
    #[must_use]
    pub const fn indicator(mut self, selector: &'a str) -> Self {
        self.indicator = Some(selector);
        self
    }

    /// Trigger a search on initial page load (useful for pre-populated results).
    #[must_use]
    pub const fn initial_load(mut self) -> Self {
        self.initial_load = true;
        self
    }

    /// Set placeholder text for the search input.
    #[must_use]
    pub const fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = Some(text);
        self
    }

    /// Set a custom query parameter name (default: `"q"`).
    #[must_use]
    pub const fn param_name(mut self, name: &'a str) -> Self {
        self.param_name = name;
        self
    }
}

/// Configuration for an [`autocomplete_input`] widget.
///
/// Build with [`AutocompleteConfig::new`] and chain builder methods for
/// optional overrides.
#[derive(Debug, Clone)]
pub struct AutocompleteConfig<'a> {
    /// URL of the server-side autocomplete handler.
    pub action: &'a str,
    /// CSS selector for an element shown while the request is in flight.
    pub indicator: Option<&'a str>,
    /// Debounce delay in milliseconds (default: `300`).
    pub debounce_ms: u32,
    /// Minimum character count before triggering autocomplete (default: `1`).
    pub min_length: u32,
    /// Query parameter name sent to the handler and used as the visible input's
    /// `name` (default: `"q"`).
    pub query_param: &'a str,
    /// `name` attribute for the hidden input storing the selected record ID.
    pub value_name: &'a str,
    /// Optional placeholder text for the visible input.
    pub placeholder: Option<&'a str>,
    /// Static `(value, label)` pairs for the `<noscript>` `<select>` fallback.
    pub fallback_options: Option<&'a [(&'a str, &'a str)]>,
}

impl<'a> AutocompleteConfig<'a> {
    /// Create a new autocomplete configuration with sensible defaults.
    ///
    /// - `action` — URL of the autocomplete handler
    /// - `value_name` — `name` attribute for the hidden selected-ID input
    #[must_use]
    pub const fn new(action: &'a str, value_name: &'a str) -> Self {
        Self {
            action,
            indicator: None,
            debounce_ms: 300,
            min_length: 1,
            query_param: "q",
            value_name,
            placeholder: None,
            fallback_options: None,
        }
    }

    /// Set the debounce delay in milliseconds (default: `300`).
    #[must_use]
    pub const fn debounce(mut self, ms: u32) -> Self {
        self.debounce_ms = ms;
        self
    }

    /// Set the minimum query length before autocomplete triggers (default: `1`).
    #[must_use]
    pub const fn min_length(mut self, n: u32) -> Self {
        self.min_length = n;
        self
    }

    /// Set a CSS selector for the htmx loading indicator element.
    #[must_use]
    pub const fn indicator(mut self, selector: &'a str) -> Self {
        self.indicator = Some(selector);
        self
    }

    /// Set a custom query parameter name (default: `"q"`).
    #[must_use]
    pub const fn query_param(mut self, name: &'a str) -> Self {
        self.query_param = name;
        self
    }

    /// Set placeholder text for the visible input.
    #[must_use]
    pub const fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = Some(text);
        self
    }

    /// Set static `(value, label)` pairs for the no-JavaScript `<select>` fallback.
    #[must_use]
    pub const fn fallback_options(mut self, options: &'a [(&'a str, &'a str)]) -> Self {
        self.fallback_options = Some(options);
        self
    }
}

/// Build the `hx-trigger` value for active search / autocomplete inputs.
///
/// The canonical pattern is:
/// `input[filter] changed delay:{n}ms, keyup[key=='Enter'][filter][, load]`
fn build_trigger(debounce_ms: u32, min_length: u32, initial_load: bool) -> String {
    let filter = if min_length > 0 {
        format!("[this.value.length >= {min_length}]")
    } else {
        String::new()
    };
    let mut trigger =
        format!("input{filter} changed delay:{debounce_ms}ms, keyup[key=='Enter']{filter}");
    // When a minimum length is configured, also fire when the value drops below
    // the threshold so stale results are cleared via the server's empty-state response.
    if min_length > 0 {
        use std::fmt::Write as _;
        let _ = write!(trigger, ", input[this.value.length < {min_length}] changed");
    }
    if initial_load {
        trigger.push_str(", load");
    }
    trigger
}

/// Strip a leading `#` from a CSS ID selector to get a bare element ID.
///
/// `aria-controls` takes an ID (no `#`), while `hx-target` takes a CSS selector.
fn selector_to_id(selector: &str) -> &str {
    selector.strip_prefix('#').unwrap_or(selector)
}

/// Render a labeled `<input type="search">` with htmx active-search attributes.
///
/// Fires debounced GET (or POST) requests as the user types and on Enter,
/// targeting `config.target`. An accessible `<label>` and `aria-controls`
/// pointing at the results container are included automatically.
///
/// Pair with [`active_search_results`] for the results container, or use
/// [`active_search`] to emit the complete widget (input + results + noscript).
///
/// # htmx attributes emitted
///
/// | Attribute | Value |
/// |-----------|-------|
/// | `hx-get` / `hx-post` | `config.action` |
/// | `hx-trigger` | `input[filter] changed delay:{n}ms, keyup[key=='Enter'][filter][, load]` |
/// | `hx-target` | `config.target` |
/// | `hx-indicator` | `config.indicator` (only when set) |
#[cfg(feature = "maud")]
#[must_use]
pub fn active_search_input(id: &str, label: &str, config: &ActiveSearchConfig<'_>) -> maud::Markup {
    let trigger = build_trigger(config.debounce_ms, config.min_length, config.initial_load);
    let aria_controls = selector_to_id(config.target);
    let (hx_get, hx_post) = match config.method {
        SearchMethod::Get => (Some(config.action), None::<&str>),
        SearchMethod::Post => (None, Some(config.action)),
    };

    maud::html! {
        div {
            label for=(id) { (label) }
            input
                type="search"
                id=(id)
                name=(config.param_name)
                autocomplete="off"
                aria-controls=(aria_controls)
                placeholder=[config.placeholder]
                hx-get=[hx_get]
                hx-post=[hx_post]
                hx-trigger=(trigger)
                hx-target=(config.target)
                hx-indicator=[config.indicator];
        }
    }
}

/// Render the results container element targeted by [`active_search_input`].
///
/// Uses `role="status"` and `aria-live="polite"` so screen readers announce
/// result updates without moving keyboard focus.
#[cfg(feature = "maud")]
#[must_use]
pub fn active_search_results(id: &str) -> maud::Markup {
    maud::html! {
        div
            id=(id)
            role="status"
            aria-live="polite"
            aria-atomic="true" {}
    }
}

/// Render a complete active search widget.
///
/// Emits:
/// - A labeled search input with htmx active-search attributes.
/// - A results container (`id="{id}-results"`).
/// - A `<noscript>` fallback form that works without JavaScript.
///
/// The results container id is `"{id}-results"`. Pass `"#{id}-results"` as
/// `config.target` to connect the input to this container.
///
/// # Example
///
/// ```rust,ignore
/// use autumn_web::widgets::{ActiveSearchConfig, active_search};
///
/// let config = ActiveSearchConfig::new("/bookmarks/search", "#bookmark-search-results")
///     .placeholder("Search bookmarks…");
///
/// html! {
///     (active_search("bookmark-search", "Search bookmarks", &config))
/// }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn active_search(id: &str, label: &str, config: &ActiveSearchConfig<'_>) -> maud::Markup {
    // Derive the results container ID from the configured target selector so the
    // rendered container always matches what the input's hx-target points at.
    let results_id = selector_to_id(config.target).to_string();
    let noscript_method = match config.method {
        SearchMethod::Get => "get",
        SearchMethod::Post => "post",
    };

    maud::html! {
        div id=(format!("{id}-wrapper")) {
            (active_search_input(id, label, config))
            (active_search_results(&results_id))
            noscript {
                form action=(config.action) method=(noscript_method) {
                    label for=(format!("{id}-noscript")) { (label) }
                    input
                        type="search"
                        id=(format!("{id}-noscript"))
                        name=(config.param_name)
                        placeholder=[config.placeholder];
                    button type="submit" { "Search" }
                }
            }
        }
    }
}

/// Render an active search empty-state partial.
///
/// Return this from your search handler when no results match the query.
/// The `role="status"` and `aria-live="polite"` attributes ensure screen
/// readers announce the empty state.
#[cfg(feature = "maud")]
#[must_use]
pub fn active_search_empty_state(message: &str) -> maud::Markup {
    maud::html! {
        div
            role="status"
            aria-live="polite"
            class="search-empty" {
            (message)
        }
    }
}

/// Render an autocomplete input widget.
///
/// Emits:
/// - A visible `<input type="search" role="combobox">` for typing.
/// - A hidden `<input>` for storing the selected record's ID.
/// - A `<div role="listbox">` where the server renders option partials.
/// - A `<noscript>` fallback `<select>`.
///
/// Use [`autocomplete_option`] and [`autocomplete_empty_state`] to render
/// option partials returned by your handler.
///
/// # Example
///
/// ```rust,ignore
/// use autumn_web::widgets::{AutocompleteConfig, autocomplete_input};
///
/// let config = AutocompleteConfig::new("/tags/autocomplete", "tag_label", "tag_id")
///     .placeholder("Search tags…");
///
/// html! {
///     (autocomplete_input("tag-picker", "Tag", &config))
/// }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn autocomplete_input(id: &str, label: &str, config: &AutocompleteConfig<'_>) -> maud::Markup {
    let query_id = format!("{id}-query");
    let value_id = format!("{id}-value");
    let options_id = format!("{id}-options");
    let trigger = build_trigger(config.debounce_ms, config.min_length, false);
    let target = format!("#{options_id}");
    // Populate the visible input and the hidden value field when a user clicks
    // an autocomplete option. Uses event delegation on the listbox container.
    let hx_on_click = format!(
        "let o=event.target.closest('[role=option]');if(o){{document.getElementById('{query_id}').value=o.textContent.trim();document.getElementById('{value_id}').value=o.getAttribute('data-value');this.innerHTML='';}}"
    );
    // Same selection logic for keyboard users: Enter or Space activates the focused option.
    let hx_on_keydown = format!(
        "let o=event.target.closest('[role=option]');if(o&&(event.key==='Enter'||event.key===' ')){{event.preventDefault();document.getElementById('{query_id}').value=o.textContent.trim();document.getElementById('{value_id}').value=o.getAttribute('data-value');this.innerHTML='';}}"
    );
    // Clear the hidden value whenever the user edits the visible field so a stale
    // selection is not submitted if they change their mind without picking again.
    let on_input_clear = format!("document.getElementById('{value_id}').value=''");

    maud::html! {
        div id=(format!("{id}-wrapper")) {
            label for=(query_id) { (label) }
            input
                type="search"
                id=(query_id)
                name=(config.query_param)
                autocomplete="off"
                role="combobox"
                aria-expanded="false"
                aria-autocomplete="list"
                aria-controls=(options_id)
                placeholder=[config.placeholder]
                oninput=(on_input_clear)
                hx-get=(config.action)
                hx-trigger=(trigger)
                hx-target=(target)
                hx-indicator=[config.indicator];
            input
                type="hidden"
                id=(value_id)
                name=(config.value_name)
                value="";
            div
                id=(options_id)
                role="listbox"
                aria-label=(label)
                aria-live="polite"
                "hx-on:click"=(hx_on_click)
                "hx-on:keydown"=(hx_on_keydown) {}
            noscript {
                select name=(config.value_name) aria-label=(label) {
                    option value="" { "— select —" }
                    @if let Some(opts) = config.fallback_options {
                        @for (val, lbl) in opts {
                            option value=(val) { (lbl) }
                        }
                    }
                }
            }
        }
    }
}

/// Render a single autocomplete option partial returned by the server.
///
/// The `data-value` attribute carries the record ID. Wire a click handler
/// via htmx (e.g. `hx-on:click`) or a minimal inline script to populate
/// the hidden field and the visible label from `data-value` and the element's
/// text content.
///
/// # Example response fragment
///
/// ```rust,ignore
/// use autumn_web::widgets::autocomplete_option;
///
/// html! {
///     @for tag in &tags {
///         (autocomplete_option(&tag.id.to_string(), &tag.name))
///     }
/// }
/// ```
#[cfg(feature = "maud")]
#[must_use]
pub fn autocomplete_option(value: &str, label: &str) -> maud::Markup {
    maud::html! {
        div
            role="option"
            tabindex="0"
            data-value=(value) {
            (label)
        }
    }
}

/// Render an autocomplete empty-state partial.
///
/// Return this from your autocomplete handler when no records match the query.
#[cfg(feature = "maud")]
#[must_use]
pub fn autocomplete_empty_state(message: &str) -> maud::Markup {
    maud::html! {
        div
            role="status"
            aria-live="polite"
            class="autocomplete-empty" {
            (message)
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "maud"))]
mod tests {
    use super::*;

    // ── build_trigger ──────────────────────────────────────────────────

    #[test]
    fn trigger_has_debounce_and_enter() {
        let t = build_trigger(300, 1, false);
        assert!(t.contains("delay:300ms"), "{t}");
        assert!(t.contains("keyup[key=='Enter']"), "{t}");
    }

    #[test]
    fn trigger_has_changed_modifier() {
        let t = build_trigger(300, 0, false);
        assert!(t.contains("changed"), "{t}");
    }

    #[test]
    fn trigger_min_length_adds_filter() {
        let t = build_trigger(300, 2, false);
        // The trigger string itself uses `>=` (not HTML-encoded); encoding happens in the template
        assert!(t.contains("[this.value.length >= 2]"), "{t}");
    }

    #[test]
    fn trigger_min_length_adds_clear_below_threshold() {
        let t = build_trigger(300, 2, false);
        assert!(t.contains("[this.value.length < 2]"), "{t}");
    }

    #[test]
    fn trigger_min_length_zero_has_no_filter() {
        let t = build_trigger(300, 0, false);
        assert!(!t.contains("this.value.length"), "{t}");
    }

    #[test]
    fn trigger_min_length_zero_has_no_clear_trigger() {
        let t = build_trigger(300, 0, false);
        assert!(!t.contains("this.value.length <"), "{t}");
    }

    #[test]
    fn trigger_initial_load_appends_load() {
        let t = build_trigger(300, 0, true);
        assert!(t.contains(", load"), "{t}");
    }

    #[test]
    fn trigger_no_initial_load_by_default() {
        let t = build_trigger(300, 0, false);
        assert!(!t.contains("load"), "{t}");
    }

    #[test]
    fn trigger_custom_debounce() {
        let t = build_trigger(750, 0, false);
        assert!(t.contains("delay:750ms"), "{t}");
    }

    // ── selector_to_id ─────────────────────────────────────────────────

    #[test]
    fn selector_to_id_strips_hash() {
        assert_eq!(selector_to_id("#my-results"), "my-results");
    }

    #[test]
    fn selector_to_id_passthrough_without_hash() {
        assert_eq!(selector_to_id("results"), "results");
    }

    // ── ActiveSearchConfig builder ─────────────────────────────────────

    #[test]
    fn config_defaults() {
        let c = ActiveSearchConfig::new("/s", "#r");
        assert_eq!(c.method, SearchMethod::Get);
        assert_eq!(c.debounce_ms, 300);
        assert_eq!(c.min_length, 1);
        assert_eq!(c.param_name, "q");
        assert!(!c.initial_load);
        assert!(c.indicator.is_none());
        assert!(c.placeholder.is_none());
    }

    #[test]
    fn config_post_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r").post().method,
            SearchMethod::Post
        );
    }

    #[test]
    fn config_debounce_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r")
                .debounce(500)
                .debounce_ms,
            500
        );
    }

    #[test]
    fn config_min_length_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r").min_length(3).min_length,
            3
        );
    }

    #[test]
    fn config_initial_load_builder() {
        assert!(
            ActiveSearchConfig::new("/s", "#r")
                .initial_load()
                .initial_load
        );
    }

    #[test]
    fn config_indicator_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r")
                .indicator("#spin")
                .indicator,
            Some("#spin")
        );
    }

    #[test]
    fn config_placeholder_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r")
                .placeholder("hint")
                .placeholder,
            Some("hint")
        );
    }

    #[test]
    fn config_param_name_builder() {
        assert_eq!(
            ActiveSearchConfig::new("/s", "#r")
                .param_name("query")
                .param_name,
            "query"
        );
    }

    // ── active_search_input ────────────────────────────────────────────

    #[test]
    fn input_defaults_to_hx_get() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains(r#"hx-get="/search""#), "{html}");
        assert!(!html.contains("hx-post"), "{html}");
    }

    #[test]
    fn input_uses_hx_post_when_configured() {
        let config = ActiveSearchConfig::new("/search", "#results").post();
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains(r#"hx-post="/search""#), "{html}");
        assert!(!html.contains("hx-get"), "{html}");
    }

    #[test]
    fn input_trigger_has_default_debounce() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("delay:300ms"), "{html}");
    }

    #[test]
    fn input_trigger_has_enter_key() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("keyup"), "{html}");
        assert!(html.contains("Enter"), "{html}");
    }

    #[test]
    fn input_configurable_debounce() {
        let config = ActiveSearchConfig::new("/search", "#results").debounce(500);
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("delay:500ms"), "{html}");
    }

    #[test]
    fn input_configurable_min_length() {
        let config = ActiveSearchConfig::new("/search", "#results").min_length(3);
        let html = active_search_input("q", "Search", &config).into_string();
        // Maud HTML-encodes `>=` as `&gt;=`; the browser decodes it before htmx sees it
        assert!(
            html.contains("this.value.length") && html.contains('3'),
            "{html}"
        );
    }

    #[test]
    fn input_no_filter_when_min_length_zero() {
        let config = ActiveSearchConfig::new("/search", "#results").min_length(0);
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(!html.contains("this.value.length"), "{html}");
    }

    #[test]
    fn input_initial_load_in_trigger() {
        let config = ActiveSearchConfig::new("/search", "#results").initial_load();
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains(", load"), "{html}");
    }

    #[test]
    fn input_no_initial_load_by_default() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(!html.contains(", load"), "{html}");
    }

    #[test]
    fn input_target_selector() {
        let config = ActiveSearchConfig::new("/search", "#my-results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("hx-target=\"#my-results\""), "{html}");
    }

    #[test]
    fn input_indicator_when_configured() {
        let config = ActiveSearchConfig::new("/search", "#results").indicator("#spinner");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("hx-indicator=\"#spinner\""), "{html}");
    }

    #[test]
    fn input_no_indicator_by_default() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(!html.contains("hx-indicator"), "{html}");
    }

    #[test]
    fn input_renders_label() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search Posts", &config).into_string();
        assert!(html.contains("Search Posts"), "{html}");
        assert!(html.contains("<label"), "{html}");
    }

    #[test]
    fn input_label_for_matches_id() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("my-search", "Search", &config).into_string();
        assert!(html.contains(r#"for="my-search""#), "{html}");
        assert!(html.contains(r#"id="my-search""#), "{html}");
    }

    #[test]
    fn input_has_aria_controls() {
        let config = ActiveSearchConfig::new("/search", "#my-results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("aria-controls"), "{html}");
        assert!(html.contains("my-results"), "{html}");
    }

    #[test]
    fn input_type_is_search() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains(r#"type="search""#), "{html}");
    }

    #[test]
    fn input_placeholder_when_configured() {
        let config = ActiveSearchConfig::new("/search", "#results").placeholder("Type to search…");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains("Type to search"), "{html}");
    }

    #[test]
    fn input_no_placeholder_by_default() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(!html.contains("placeholder"), "{html}");
    }

    #[test]
    fn input_custom_param_name() {
        let config = ActiveSearchConfig::new("/search", "#results").param_name("query");
        let html = active_search_input("q", "Search", &config).into_string();
        assert!(html.contains(r#"name="query""#), "{html}");
    }

    // ── active_search_results ──────────────────────────────────────────

    #[test]
    fn results_correct_id() {
        let html = active_search_results("my-results").into_string();
        assert!(html.contains(r#"id="my-results""#), "{html}");
    }

    #[test]
    fn results_role_status() {
        let html = active_search_results("r").into_string();
        assert!(html.contains(r#"role="status""#), "{html}");
    }

    #[test]
    fn results_aria_live_polite() {
        let html = active_search_results("r").into_string();
        assert!(html.contains(r#"aria-live="polite""#), "{html}");
    }

    #[test]
    fn results_aria_atomic() {
        let html = active_search_results("r").into_string();
        assert!(html.contains(r#"aria-atomic="true""#), "{html}");
    }

    // ── active_search (full widget) ────────────────────────────────────

    #[test]
    fn widget_includes_input_and_results() {
        let config = ActiveSearchConfig::new("/search", "#s-results");
        let html = active_search("s", "Search", &config).into_string();
        assert!(html.contains(r#"type="search""#), "{html}");
        assert!(html.contains(r#"id="s-results""#), "{html}");
    }

    #[test]
    fn widget_results_id_matches_target_selector() {
        // Callers pass any target; the generated container must match.
        let config = ActiveSearchConfig::new("/search", "#custom-results");
        let html = active_search("search-widget", "Search", &config).into_string();
        assert!(html.contains(r#"id="custom-results""#), "{html}");
    }

    #[test]
    fn widget_has_noscript_fallback() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search("s", "Search", &config).into_string();
        assert!(html.contains("<noscript>"), "{html}");
        assert!(html.contains("<form"), "{html}");
        assert!(html.contains(r#"type="submit""#), "{html}");
    }

    #[test]
    fn widget_noscript_get_by_default() {
        let config = ActiveSearchConfig::new("/search", "#results");
        let html = active_search("s", "Search", &config).into_string();
        assert!(html.contains(r#"method="get""#), "{html}");
    }

    #[test]
    fn widget_noscript_post_when_configured() {
        let config = ActiveSearchConfig::new("/search", "#results").post();
        let html = active_search("s", "Search", &config).into_string();
        assert!(html.contains(r#"method="post""#), "{html}");
    }

    // ── autocomplete_input ─────────────────────────────────────────────

    #[test]
    fn autocomplete_visible_search_input() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"type="search""#), "{html}");
        // visible input uses query_param (default "q") so htmx sends ?q=...
        assert!(html.contains(r#"name="q""#), "{html}");
    }

    #[test]
    fn autocomplete_visible_input_uses_query_param() {
        let config = AutocompleteConfig::new("/ac", "value_field").query_param("search");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"name="search""#), "{html}");
    }

    #[test]
    fn autocomplete_hidden_value_field() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"type="hidden""#), "{html}");
        assert!(html.contains(r#"name="value_field""#), "{html}");
    }

    #[test]
    fn autocomplete_hidden_field_empty_initial_value() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"type="hidden""#), "{html}");
        assert!(html.contains(r#"value="""#), "{html}");
    }

    #[test]
    fn autocomplete_listbox_container() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"role="listbox""#), "{html}");
    }

    #[test]
    fn autocomplete_listbox_has_click_handler() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("hx-on:click"), "{html}");
    }

    #[test]
    fn autocomplete_combobox_role() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"role="combobox""#), "{html}");
    }

    #[test]
    fn autocomplete_aria_expanded_false() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"aria-expanded="false""#), "{html}");
    }

    #[test]
    fn autocomplete_aria_autocomplete_list() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"aria-autocomplete="list""#), "{html}");
    }

    #[test]
    fn autocomplete_has_aria_controls() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("aria-controls"), "{html}");
    }

    #[test]
    fn autocomplete_renders_label() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "My Label", &config).into_string();
        assert!(html.contains("My Label"), "{html}");
        assert!(html.contains("<label"), "{html}");
    }

    #[test]
    fn autocomplete_label_for_matches_query_input_id() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("tag", "Tag", &config).into_string();
        assert!(html.contains(r#"for="tag-query""#), "{html}");
        assert!(html.contains(r#"id="tag-query""#), "{html}");
    }

    #[test]
    fn autocomplete_has_noscript_fallback() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("<noscript>"), "{html}");
        assert!(html.contains("<select"), "{html}");
    }

    #[test]
    fn autocomplete_noscript_select_uses_value_name() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"name="value_field""#), "{html}");
    }

    #[test]
    fn autocomplete_fallback_options_rendered_in_noscript() {
        let opts: &[(&str, &str)] = &[("1", "Alpha"), ("2", "Beta")];
        let config = AutocompleteConfig::new("/ac", "value_field").fallback_options(opts);
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("Alpha"), "{html}");
        assert!(html.contains("Beta"), "{html}");
        assert!(html.contains(r#"value="1""#), "{html}");
        assert!(html.contains(r#"value="2""#), "{html}");
    }

    #[test]
    fn autocomplete_has_hx_get() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains(r#"hx-get="/ac""#), "{html}");
    }

    #[test]
    fn autocomplete_hx_trigger_has_debounce() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("hx-trigger"), "{html}");
        assert!(html.contains("delay:300ms"), "{html}");
    }

    #[test]
    fn autocomplete_configurable_debounce() {
        let config = AutocompleteConfig::new("/ac", "value_field").debounce(600);
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("delay:600ms"), "{html}");
    }

    #[test]
    fn autocomplete_configurable_min_length() {
        let config = AutocompleteConfig::new("/ac", "value_field").min_length(2);
        let html = autocomplete_input("x", "Label", &config).into_string();
        // Maud HTML-encodes `>=` as `&gt;=`; the browser decodes it before htmx sees it
        assert!(
            html.contains("this.value.length") && html.contains('2'),
            "{html}"
        );
    }

    #[test]
    fn autocomplete_indicator_when_configured() {
        let config = AutocompleteConfig::new("/ac", "value_field").indicator("#ld");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("hx-indicator=\"#ld\""), "{html}");
    }

    #[test]
    fn autocomplete_no_indicator_by_default() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(!html.contains("hx-indicator"), "{html}");
    }

    #[test]
    fn autocomplete_listbox_has_aria_live() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("aria-live"), "{html}");
    }

    #[test]
    fn autocomplete_listbox_has_keyboard_handler() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("hx-on:keydown"), "{html}");
        assert!(html.contains("Enter"), "{html}");
    }

    #[test]
    fn autocomplete_visible_input_clears_hidden_on_change() {
        let config = AutocompleteConfig::new("/ac", "value_field");
        let html = autocomplete_input("x", "Label", &config).into_string();
        assert!(html.contains("oninput"), "{html}");
    }

    // ── autocomplete_option ────────────────────────────────────────────

    #[test]
    fn option_renders_label_and_value() {
        let html = autocomplete_option("42", "My Tag").into_string();
        assert!(html.contains("My Tag"), "{html}");
        assert!(html.contains(r#"data-value="42""#), "{html}");
    }

    #[test]
    fn option_has_role_option() {
        let html = autocomplete_option("1", "Option").into_string();
        assert!(html.contains(r#"role="option""#), "{html}");
    }

    #[test]
    fn option_is_keyboard_focusable() {
        let html = autocomplete_option("1", "Option").into_string();
        assert!(html.contains("tabindex"), "{html}");
    }

    // ── autocomplete_empty_state ───────────────────────────────────────

    #[test]
    fn ac_empty_state_renders_message() {
        let html = autocomplete_empty_state("No results found").into_string();
        assert!(html.contains("No results found"), "{html}");
    }

    #[test]
    fn ac_empty_state_announced_to_screen_readers() {
        let html = autocomplete_empty_state("No results").into_string();
        assert!(
            html.contains(r#"role="status""#) || html.contains("aria-live"),
            "{html}"
        );
    }

    // ── active_search_empty_state ──────────────────────────────────────

    #[test]
    fn search_empty_state_renders_message() {
        let html = active_search_empty_state("No matching posts").into_string();
        assert!(html.contains("No matching posts"), "{html}");
    }

    #[test]
    fn search_empty_state_announced_to_screen_readers() {
        let html = active_search_empty_state("Nothing found").into_string();
        assert!(
            html.contains(r#"role="status""#) || html.contains("aria-live"),
            "{html}"
        );
    }
}
