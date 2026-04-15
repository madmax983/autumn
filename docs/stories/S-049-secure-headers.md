# 🔭 Vantage: Spec for Secure Headers

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-05
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want security-related HTTP headers to be set by default on all responses, so that my application is protected against common web vulnerabilities (like MIME-type sniffing and clickjacking) without me having to manually configure them.

---

## The "So What?" Ask

What business problem does this solve?
Most developers don't know the full list of modern security headers required to pass an automated security audit or protect their users. If Autumn doesn't provide secure defaults, applications built with the framework will be vulnerable out of the box. By providing "secure-by-default" headers, we prevent severe vulnerabilities and significantly decrease the time it takes for a team to pass security reviews and ship to production, cementing Autumn as a production-ready framework.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Provides a comprehensive set of default security headers (e.g., `X-Content-Type-Options`, `X-XSS-Protection`, `Cache-Control`, `X-Frame-Options`, `Strict-Transport-Security`).
- **Loco / Other Rust Frameworks:** Often require developers to manually add tower-http's `SetResponseHeaderLayer` or similar middleware to configure each header manually.
- **Our Gap:** Autumn currently does not apply basic security headers to its responses by default. Developers have to explicitly wire in standard security middlewares, which violates our convention-over-configuration goal for production readiness.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must set `X-Content-Type-Options: nosniff` on all responses.
- Must set `X-Frame-Options: DENY` by default, but allow it to be configurable.
- Must set `Strict-Transport-Security` automatically when running in the `production` profile.
- Must set a `Content-Security-Policy` with sensible defaults that allow `htmx` to function normally.
- Must allow all of these headers to be overridden or disabled globally via `autumn.toml`.

---

## Metric Definition

Success = 100% of newly generated Autumn applications pass standard security header scanners (like Mozilla Observatory) out of the box, with <1ms overhead added to request processing times.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic generation of dynamic CSP nonces for inline scripts (Phase 2).
- Route-specific header configuration (only global configuration via `autumn.toml` is in scope for Phase 1).
