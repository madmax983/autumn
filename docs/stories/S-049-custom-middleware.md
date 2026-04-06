# 🔭 Vantage: Spec for Custom Middleware

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-03-31
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to add custom Tower middleware to the Autumn application, so that I can implement cross-cutting concerns like custom authentication, advanced tracing, or third-party integrations that aren't natively supported by the framework.

---

## The "So What?" Ask

What business problem does this solve?
While Autumn provides strong defaults and built-in middleware for common concerns, business applications inevitably require bespoke logic applied across all requests (e.g., proprietary request signing, legacy system header injection, or integrating specialized telemetry). If developers cannot easily drop down to the standard `tower::Layer` ecosystem, they will be forced to fork the framework or abandon it. Supporting custom middleware ensures Autumn remains extensible and relevant for enterprise use cases.

---

## Gap Analysis

Look at the market:
- **Axum / Tower:** Natively built around `Service` and `Layer` abstractions, making middleware trivial.
- **Actix Web:** Has a robust `wrap` method for applying middleware.
- **Our Gap:** Autumn currently abstracts away the Axum `Router` entirely. There is no supported API to insert a `tower::Layer` into the middleware stack, locking developers out of the rich ecosystem of existing Tower middleware.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must expose a `.layer()` method on the `AutumnApp` builder.
- Must accept standard `tower::Layer` implementations.
- Must ensure custom middleware integrates correctly with Autumn's built-in middleware (e.g., executing after request ID generation so custom middleware can log the request ID).
- Must document the exact ordering of the middleware stack (which layers wrap which).
- Must verify that `Poll::Ready` / `Poll::Pending` backpressure propagates correctly through the middleware stack.

---

## Metric Definition

Success = A developer can add a standard Tower timeout layer to their Autumn app with one line of code, and integration tests confirm it triggers correctly.

---

## Out of Scope

🚫 **Out of Scope:**
- Route-specific middleware (for Phase 1, only global middleware applied to the entire app is supported).
- Creating new, framework-specific middleware traits (must use standard `tower::Layer`).
