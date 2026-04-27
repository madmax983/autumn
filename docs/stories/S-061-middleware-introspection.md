# 🔭 Vantage: Spec for Middleware Introspection

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-21
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As a Framework Plugin Developer, I want to introspect the registered middleware layers in an Autumn application before it starts, so that I can automatically detect misconfigurations (like authentication being registered after rate-limiting) and emit helpful warnings or errors at startup.

---

## The "So What?" Ask

What business problem does this solve?
Middleware ordering bugs are notoriously difficult to debug, often leading to subtle security vulnerabilities (e.g., rate limits bypassing unauthenticated requests) or performance issues. Currently, Autumn registers custom middleware as opaque closures, meaning plugins have zero visibility into what has been added or in what order. By exposing type metadata (`TypeId`) for registered layers, we empower the ecosystem to provide guardrails, reducing runtime incidents and improving the overall developer experience.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Has a highly inspectable filter and interceptor chain, often relying on `@Order` annotations and dependency injection to catch misconfigurations early.
- **Tower / Axum:** Developers explicitly construct the `ServiceBuilder` stack, making order visible in source code, but opaque to external tooling or plugins wrapping the application.
- **Our Gap:** Autumn abstracts the `Router` creation via `.layer()`, storing layers as pure `Box<dyn FnOnce(...)>`. There is currently no way to ask the framework, "Is `AuthLayer` already registered before I add `RateLimitLayer`?"

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must modify the internal storage of custom layers to retain a `TypeId` alongside the application closure.
- Must expose an API on `AutumnApp` (e.g., `.has_layer::<T>()` or `.get_layer_types()`) to query registered middleware types before `.run()` is called.
- Must ensure that this abstraction remains a zero-cost abstraction at runtime (only affecting application bootstrap).
- Must verify through tests that a plugin can successfully detect the presence or absence of a specific `tower::Layer` type in the custom layers list.
- Must ensure backwards compatibility with the existing `.layer()` API, without requiring developers to change how they register standard middleware.

---

## Metric Definition

Success = A plugin developer can write a pre-flight check that reliably panics with a clear error message if `AuthLayer` is missing, adding <2ms to the application startup time.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic reordering of middleware (the framework should warn or error, not magically reorder user-supplied logic).
- Introspection of built-in framework middleware (e.g., logging, CORS), which are already handled predictably by the core routing logic.
