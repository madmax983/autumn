# Problem Details JSON Errors Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Standardize framework-generated JSON/API errors on one Problem Details contract.

**Architecture:** `AutumnError` becomes the canonical Problem Details source. The router exception-filter path enriches request-aware fields and preserves the existing HTML error-page negotiation, while Autumn extractor wrappers convert parser rejections into the same contract.

**Tech Stack:** Rust 2024, Axum/Tower middleware, serde/serde_json, OpenAPI 3.1, cargo test.

---

### Task 1: RED Contract Tests

**Files:**
- Create: `autumn/tests/problem_details.rs`
- Modify: `autumn/tests/openapi.rs`
- Create: `docs/schemas/problem-details.schema.json`

**Steps:**
1. Add tests that assert Problem Details fields and `application/problem+json` for 400, 401, 403/404, 409, 422, 500, and 503 cases.
2. Add tests for parser rejections (`Json`, `Path`, `Query`), validation details, CSRF, fallback 404, and prod-safe 500 details.
3. Run `cargo test -p autumn-web --test problem_details` and confirm failures describe the old envelope or text/plain bodies.

### Task 2: GREEN Core Error Contract

**Files:**
- Modify: `autumn/src/error.rs`
- Modify: `autumn/src/middleware/exception_filter.rs`
- Modify: `autumn/src/middleware/error_page_filter.rs`

**Steps:**
1. Replace the legacy `{"error": ...}` body with a typed Problem Details body.
2. Preserve stable machine `code`, `request_id`, and field-level `errors` extension fields.
3. Add request-aware response enrichment without breaking HTML error pages.
4. Run the focused tests until core errors pass.

### Task 3: GREEN Extractors and CSRF

**Files:**
- Modify: `autumn/src/extract.rs`
- Modify: `autumn/src/validation.rs`
- Modify: `autumn/src/security/csrf.rs`

**Steps:**
1. Introduce Autumn wrapper extractors for `Json`, `Form`, `Path`, and `Query`.
2. Map Axum rejections into `AutumnError` with matching status codes.
3. Preserve validation field details in the canonical `errors` extension.
4. Return CSRF failures as Problem Details JSON.

### Task 4: Docs and OpenAPI

**Files:**
- Modify: `autumn/src/openapi.rs`
- Modify: `docs/guide/tutorial/09-errors.md`
- Modify: `docs/guide/what-happens-when.md`

**Steps:**
1. Register a reusable `ProblemDetails` schema in generated OpenAPI components.
2. Add standard error responses that reference the shared schema.
3. Update active docs with the contract, matrix examples, dev/prod behavior, and migration note.

### Task 5: Refactor and Verify

**Files:**
- Touch only files above unless a compiler error reveals a direct dependent site.

**Steps:**
1. Run `cargo fmt`.
2. Run focused tests for `problem_details`, `extractors`, `openapi`, and existing error-page middleware tests.
3. Scan affected areas for TODO/FIXME/stubs.
4. Review the diff for one coherent unit of work.
