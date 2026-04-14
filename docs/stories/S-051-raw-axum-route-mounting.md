# 🔭 Vantage: Spec for Raw Axum Route Mounting

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Status:** Not Started
**Created:** 2026-04-10
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Advanced Application Developer, I want to be able to mount standard raw Axum routers alongside my Autumn-annotated routes, so that I can implement advanced routing needs (like integrating third-party Axum ecosystem crates, GraphQL endpoints, or legacy services) without giving up the framework's core benefits for the rest of my app.

---

## The "So What?" Ask

What business problem does this solve?
Frameworks built on convention-over-configuration often hit a "cliff" where falling off the golden path means having to eject from the framework entirely. By providing a clean escape hatch down to the underlying `axum::Router`, developers are never completely blocked by what Autumn's macro system can express. This prevents "framework lock-in" objections during technology evaluations, boosting enterprise adoption and long-term viability of the framework.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Has robust facilities to bypass its MVC abstraction and define raw servlets or handler mappings when necessary.
- **Loco / Cot:** While some provide route injection, they often obscure the underlying primitives, making integration with existing ecosystem tooling a headache.
- **Our Gap:** Autumn generates Axum routers internally, but we need an explicit, supported API that allows developers to compose their own custom Axum `Router` directly into the Autumn `AppBuilder` pipeline, ensuring shared state and middleware still apply.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must expose a `.merge()` method on the `AutumnApp` builder.
- Must accept a standard `axum::Router`.
- Must ensure merged Axum routes have access to the same shared application state (e.g., database pool, config).
- Must ensure that global Autumn middleware (like request ID generation and logging) applies to the merged routes just as it does to macro-annotated routes.
- Must explicitly document when and how to use this escape hatch versus native Autumn route annotations.

---

## Metric Definition

Success = An engineering team can mount a pre-built `async-graphql` Axum router into an Autumn application in under 3 lines of code, and integration tests show the endpoint works and correctly receives the standard `X-Request-Id` response header.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic two-way translation between Autumn extractors and raw Axum extractors (developers writing raw Axum routes must use standard Axum extractors).
- Modifying or wrapping the Axum `Router` type itself.