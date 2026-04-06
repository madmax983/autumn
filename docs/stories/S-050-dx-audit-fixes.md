# 🔭 Vantage: Spec for DX Audit Fixes

## Description

* 👤 **User Story:** As a developer learning the Autumn framework, I want a frictionless onboarding experience with clear error messages, complete prelude imports, and simple documentation, so that I don't waste time deciphering jargon, typing long import paths, or debugging cryptic type errors.
* ✅ **Acceptance Criteria:**
  - The `Path` and `Query` extractors must be exported in the `autumn_web::prelude` module.
  - Invalid route handlers must produce clear, actionable error messages (e.g., via a restored `debug_handler` wrapper or framework equivalent) instead of complex Axum type-bound errors.
  - The `README.md` and other core documentation must replace the term "Hybrid rendering" with "Pre-rendering pages to static HTML" (or similar clear language).
  - The `README.md` and other core documentation must replace the term "Escape hatches" with "Customization options" (or similar clear language).
* 🚫 **Out of Scope:**
  - Rewriting the underlying `axum::debug_handler` logic (rely on existing ecosystem tools or simple wrappers).
  - A complete rewrite of the framework documentation.
  - Changing the internal route registration mechanism.
