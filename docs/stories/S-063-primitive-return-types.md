# 🔭 Vantage: Spec for Primitive Return Types

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 3
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-22
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to be able to return primitive types (like `i32`, `bool`, etc.) directly from my route handlers, so that I can write simple endpoints without needing to manually convert them to Strings or JSON objects, avoiding massive compiler errors.

---

## The "So What?" Ask

What business problem does this solve?
When new developers try a simple endpoint returning a primitive like `42`, they are met with a complex, 20-line compiler error about Axum internal traits (`Handler` not satisfied, `IntoResponse` missing). This creates a steep learning curve and breaks the illusion of a simple web framework. Providing out-of-the-box support for returning standard primitives reduces onboarding friction, accelerates simple development tasks, and improves the overall Developer Experience (DX) by making the compiler "just work".

---

## Gap Analysis

Look at the market:
- **Axum (Underlying Framework):** Does not implement `IntoResponse` for primitives directly by default, forcing users to convert to `String` or wrap in `Json`.
- **Actix Web:** Supports returning various types with implicit conversions or simple wrappers.
- **Our Gap:** Autumn currently exposes Axum's lack of primitive support directly to the user. We need to bridge this gap by either implementing a trait or providing a seamless macro-level transformation so that returning an `i32` or similar primitive is as easy as returning a `&str`.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must allow returning standard primitive numerical types (e.g., `i32`, `u32`, `i64`, `u64`, `f32`, `f64`) directly from a route handler.
- Must allow returning boolean types (`bool`) directly from a route handler.
- Must serialize these primitives into plain text responses by default.
- Must not introduce any breaking changes to existing handlers returning strings, JSON, or custom response types.
- Must eliminate the massive Axum-internal compiler errors previously seen when attempting to return these primitives.

---

## Metric Definition

Success = A user can write `async fn foo() -> i32 { 42 }`, it compiles successfully without any Axum trait errors, and the endpoint returns `42` as an HTTP response.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic JSON serialization of primitives (e.g., returning `{ "value": 42 }` implicitly); they should be plain text.
- Implementing primitive support for complex nested structures without explicit JSON wrapping.
