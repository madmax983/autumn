# 🔭 Vantage: Spec for DX Audit Fixes

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Must Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-25
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer new to the Autumn framework, I want intuitive default behaviors, helpful compiler errors, and a complete set of ergonomic extractors out-of-the-box, so that my first 60 seconds with the framework feel magical rather than frustrating.

---

## The "So What?" Ask

What business problem does this solve?
Developer Experience (DX) is the primary driver of framework adoption. Multiple recent DX Audits (Echo) have revealed significant onboarding friction:
1. "Leaky" proc-macro errors exposing internal functions like `__autumn_route_info_missing_route`.
2. Bizarre `content-length: 0` responses on 404s instead of a default error payload.
3. The lack of standard HTTP 500 error constructors (`internal_server_error`) forcing verbose, clunky instantiations.
4. Core extractors like `Path` and `Query` missing from `autumn_web::prelude`, creating noisy imports.
5. `autumn new` generating projects that panic due to missing optional tools (Tailwind CSS CLI).
6. Missing primitive return type support (e.g., returning `42` crashes with Axum internals).

These sharp edges erode trust immediately. Fixing these consolidates the "it just works" experience, reducing onboarding time, lowering the support burden, and improving conversion from "trying it out" to "building a production app." Complexity is a cost; utility is revenue.

---

## Gap Analysis

Look at the market:
- **Axum:** Opts for explicit imports and detailed, but sometimes overwhelming, trait bounds.
- **Actix Web:** Offers a highly ergonomic prelude and graceful error handling out-of-the-box.
- **Spring Boot:** Famous for its "Whitelabel Error Page" and out-of-the-box auto-configuration that rarely fails on simple mistakes.
- **Our Gap:** Autumn currently leaks its Axum underpinnings during simple mistakes (macro typos, primitive returns). It forces too much typing for standard web features (Path/Query) and fails ungracefully (empty 404s, Tailwind panics).

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must re-export `Path` and `Query` extractors within `autumn_web::prelude`.
- Must suppress or cleanly handle secondary compiler errors (`__autumn_route_info_...`) when a route macro contains a typo.
- Must provide `internal_server_error` and `internal_server_error_msg` constructors on `AutumnError`.
- Must ensure that unhandled 404 routes return a default payload (HTML or JSON based on accept headers) rather than an empty `content-length: 0` response.
- Must ensure `autumn dev` gracefully degrades (with a warning, not a panic) if the Tailwind CLI is missing, allowing the application to run without CSS compilation.
- Must allow returning standard primitive types (e.g., `i32`) from route handlers without verbose trait-bound compiler errors (integrating S-063).
- Must ensure generated templates (`autumn new`) explicitly use the workspace configuration to prevent compilation errors when generated inside the main repository.

---

## Metric Definition

Success = A new user can run `autumn new app`, modify the default handler to return `i32`, use `Path` without extra imports, intentionally trigger a 500 using `AutumnError::internal_server_error`, hit a non-existent route to see a styled 404 page, and have it all compile and run smoothly without the Tailwind CLI installed.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic downloading of the Tailwind CLI without explicit user opt-in (`autumn setup`).
- Completely rewriting the macro engine; we only need to catch and suppress the specific `__autumn_route_info` leakage.
- Advanced runtime error analytics or dashboarding.
