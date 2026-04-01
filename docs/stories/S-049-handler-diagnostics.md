# 🔭 Vantage: Spec for Improved Error Diagnostics for Route Handlers

**Epic:** EPIC-012 (Developer Experience & Onboarding)
**Priority:** Must Have
**Story Points:** 3
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-01
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As a Developer building endpoints, I want clear, readable compiler errors when my route handler signature is invalid, so that I can quickly fix my mistakes without parsing deep framework internals.

---

## The "So What?" Ask

What business problem does this solve?
Currently, if a developer forgets an extractor (e.g., using `String` instead of `Path<String>`), they are hit with massive, unreadable Axum `Handler<_, _>` type-bound errors. Time spent deciphering these errors is time not spent shipping features. Fixing this drastically reduces time-to-first-successful-compile, lowers the learning curve for new users, and prevents developer churn caused by "terrible error messages". It turns a painful failure mode into a helpful learning moment.

---

## Gap Analysis

Look at the market:
- **Axum (Standard):** Provides the `#[debug_handler]` macro which specifically intercepts bad signatures and outputs targeted, human-readable compiler errors.
- **Loco / Other Frameworks:** Typically expose `#[debug_handler]` directly or rely on standard Axum behavior.
- **Our Gap:** Autumn currently removes `#[axum::debug_handler]` from our route macros (like `#[get]`) to avoid module path resolution errors for users who haven't explicitly added `axum` to their `Cargo.toml`. This solved one problem but created a much worse one: we robbed the developer of their primary diagnostic tool for handler errors.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must provide targeted, readable compiler errors when a handler parameter does not implement the required extractor traits.
- Must not re-introduce the `::axum::` module path resolution errors that caused `debug_handler` to be removed in the first place (i.e., it must work even if the user only depends on `autumn-web`).
- Must work out-of-the-box for all Autumn route macros (`#[get]`, `#[post]`, etc.) without requiring the developer to add manual annotations.
- Must not significantly increase compile times for valid routes.

---

## Metric Definition

Success = A developer using an invalid parameter type (like `i32` instead of `Path<i32>`) in a route handler receives an error message explicitly pointing to the invalid type, and the total error output is <5 lines long, rather than a page of trait resolution failures.

---

## Out of Scope

🚫 **Out of Scope:**
- Writing a completely custom Rust compiler frontend or building a dedicated rust-analyzer plugin.
- Implementing runtime type checking for handlers.
- Discussing or dictating the specific engineering implementation details (e.g., whether to use a custom macro wrapper, an Axum re-export, or another macro trick). That is Engineering's job.
