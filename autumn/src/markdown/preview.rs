//! HTMX Live Preview Handler for Markdown.

use crate::extract::Form;
use crate::markdown::{RenderOptions, render};
#[cfg(feature = "maud")]
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;

/// Form input for the live preview handler.
#[derive(Debug, Deserialize)]
pub struct MarkdownInput {
    /// The raw markdown string to render.
    pub markdown: String,
}

/// HTMX handler for live Markdown preview.
///
/// Accepts a `POST` request containing a url-encoded form with a `markdown`
/// field. It parses the markdown and returns the rendered HTML wrapped
/// in a generic container.
///
/// # Example (HTMX Client)
///
/// ```html
/// <form hx-post="/docs/preview" hx-target="#preview" hx-trigger="keyup delay:500ms from:#markdown-input">
///     <textarea id="markdown-input" name="markdown"># Hello</textarea>
/// </form>
/// <div id="preview"></div>
/// ```
#[cfg(feature = "maud")]
pub async fn live_preview(Form(input): Form<MarkdownInput>) -> Markup {
    let rendered = render(&input.markdown, RenderOptions::default());
    html! {
        div class="markdown-preview" {
            (PreEscaped(rendered.html))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "maud")]
    #[tokio::test]
    async fn test_live_preview_renders_markdown() {
        let input = MarkdownInput {
            markdown: "# Hello\n\nThis is **bold** text.".to_string(),
        };
        let markup = live_preview(Form(input)).await;
        let html = markup.into_string();

        assert!(html.contains("<div class=\"markdown-preview\">"));
        assert!(html.contains("<h1 id=\"hello\">Hello</h1>"));
        assert!(html.contains("<p>This is <strong>bold</strong> text.</p>"));
    }
}
