//! Design tokens shared by framework-owned HTML surfaces.
//!
//! Embed [`TOKENS_CSS`] inside a `<style>` tag; subsequent rules can
//! reference the custom properties via `var(--bg)`, `var(--primary)`, etc.
//!
//! The token set is light-mode only — surfaces that need dark mode should
//! add their own `@media (prefers-color-scheme: dark)` overrides.
//!
//! # Example
//!
//! ```rust
//! use autumn_web::ui::tokens::TOKENS_CSS;
//!
//! let stylesheet = format!("{TOKENS_CSS}\nbody {{ background: var(--bg); }}");
//! assert!(stylesheet.contains("--bg"));
//! ```

/// `:root` CSS custom properties: colors, radius, shadow, and font stack.
///
/// Emitted inside a `<style>` block so downstream rules can reference the
/// variables. Sourced from `tokens.css` so `include_str!` can share the
/// same literal with places that need `concat!`-time composition (e.g. the
/// error-pages fallback stylesheet).
pub const TOKENS_CSS: &str = include_str!("tokens.css");

/// Flash-message CSS consuming the shared tokens.
///
/// Paired with [`crate::flash::FlashLevel::as_str`] (`"success"` /
/// `"error"` / `"warning"` / `"info"`). Render a flash like:
///
/// ```html
/// <div class=\"flash flash-success\">Saved.</div>
/// ```
pub const FLASH_CSS: &str = "\
.flash {
    padding: 0.75rem 1rem;
    border-radius: 0.375rem;
    margin-bottom: 1rem;
    font-size: 0.875rem;
}
.flash-success { background: var(--success-light); color: var(--success); border: 1px solid var(--success); }
.flash-error { background: var(--danger-light); color: var(--danger); border: 1px solid var(--danger); }
.flash-warning { background: var(--warning-light); color: var(--warning); border: 1px solid var(--warning); }
.flash-info { background: var(--primary-light); color: var(--primary); border: 1px solid var(--primary); }
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_defines_expected_vars() {
        for v in [
            "--bg",
            "--surface",
            "--text",
            "--text-muted",
            "--border",
            "--primary",
            "--danger",
            "--success",
            "--warning",
            "--radius",
            "--shadow",
            "--font-family",
        ] {
            assert!(TOKENS_CSS.contains(v), "TOKENS_CSS is missing {v}");
        }
    }

    #[test]
    fn flash_uses_shared_tokens() {
        for v in [
            "var(--success)",
            "var(--danger)",
            "var(--warning)",
            "var(--primary)",
        ] {
            assert!(FLASH_CSS.contains(v), "FLASH_CSS should reference {v}");
        }
    }
}
