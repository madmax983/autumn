//! First-class Markdown rendering with frontmatter parsing and SSG integration.
//!
//! Enable with the Cargo feature `markdown`.
//!
//! ## Quick start
//!
//! ### 1. Embed Markdown files at compile time
//!
//! ```rust,ignore
//! use std::sync::OnceLock;
//! use autumn_web::markdown::{MarkdownRegistry, MarkdownSource, RenderOptions, render};
//!
//! static DOCS: OnceLock<MarkdownRegistry> = OnceLock::new();
//!
//! fn docs() -> &'static MarkdownRegistry {
//!     DOCS.get_or_init(|| {
//!         MarkdownRegistry::from_embedded(&[
//!             MarkdownSource { slug: "intro", content: include_str!("../content/intro.md") },
//!             MarkdownSource { slug: "api",   content: include_str!("../content/api.md") },
//!         ]).expect("embedded docs are valid")
//!     })
//! }
//! ```
//!
//! ### 2. Render a page dynamically
//!
//! ```rust,ignore
//! #[get("/docs/{slug}")]
//! async fn show_doc(Path(slug): Path<String>) -> AutumnResult<Markup> {
//!     let page = docs().get(&slug)
//!         .ok_or_else(|| AutumnError::not_found())?;
//!     let out = render(&page.body, RenderOptions::default());
//!     Ok(layout(&page.frontmatter.title, html! {
//!         (PreEscaped(&out.html))
//!     }))
//! }
//! ```
//!
//! ### 3. Wire up static pre-rendering
//!
//! ```rust,ignore
//! async fn doc_params(_router: axum::Router) -> Vec<StaticParams> {
//!     docs().static_params()
//! }
//!
//! #[static_get("/docs/{slug}", params = doc_params)]
//! async fn show_doc_static(Path(slug): Path<String>) -> AutumnResult<Markup> {
//!     // same as the dynamic handler above
//! }
//!
//! // In main():
//! autumn_web::app()
//!     .routes(routes![show_doc_static, ...])
//!     .static_routes(static_routes![show_doc_static])
//!     .run()
//!     .await;
//! ```
//!
//! ## Frontmatter format
//!
//! Each `.md` file must begin with a TOML block enclosed in `+++` delimiters:
//!
//! ```text
//! +++
//! title = "Getting Started"
//! description = "Set up your app in minutes."
//! order = 1
//! +++
//!
//! # Getting Started
//!
//! ...
//! ```
//!
//! The `title` field is required; `description` and `order` are optional
//! (defaulting to `""` and `0` respectively).

mod preview;
mod registry;
mod renderer;
mod types;

#[cfg(feature = "maud")]
pub use preview::{MarkdownInput, live_preview};
pub use registry::MarkdownRegistry;
pub use renderer::{heading_id, render};
pub use types::{
    MarkdownError, MarkdownFrontmatter, MarkdownPage, MarkdownSource, RenderOptions,
    RenderedMarkdown, TocItem,
};
