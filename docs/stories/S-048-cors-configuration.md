# 🔭 Vantage: Spec for CORS Configuration

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-03-31
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want declarative CORS configuration via the framework's standard configuration file, so that I can securely enable cross-origin requests for my APIs without writing custom middleware.

---

## The "So What?" Ask

What business problem does this solve?
Cross-Origin Resource Sharing (CORS) is notoriously tricky to get right. If we don't handle it for the developer, they will spend hours fighting browser preflight requests and likely end up writing permissive, insecure middleware just to make things work. By providing sensible defaults (permissive in dev, locked down in prod) and declarative configuration, we eliminate a major friction point in building web applications and save developers time.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Offers robust `@CrossOrigin` annotations and global CORS configuration that are highly integrated.
- **Loco / Other Rust Frameworks:** Require importing `tower-http` and manually wiring up CORS middleware, which often leads to verbose setup code.
- **Our Gap:** Autumn currently does not apply CORS headers by default, meaning developers must manually drop down to raw Axum/Tower configurations to serve frontend SPAs or external API consumers.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must apply CORS middleware automatically to the application router.
- Must default to a permissive policy (allow all origins) in the `development` profile.
- Must default to a restrictive policy (same-origin only, no CORS) in the `production` profile.
- Must allow configuring allowed origins, methods, and headers via `autumn.toml`.
- Must support environment variable overrides (e.g., `AUTUMN_CORS__ALLOWED_ORIGINS`).

---

## Metric Definition

Success = Developers can enable CORS for specific origins simply by updating `autumn.toml` without writing any Rust code, and preflight requests resolve in <1ms overhead.

---

## Out of Scope

🚫 **Out of Scope:**
- Route-specific CORS configuration (global only for Phase 1).
- Dynamic CORS policies that query a database to determine allowed origins.
