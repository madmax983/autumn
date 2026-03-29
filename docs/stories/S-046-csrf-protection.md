# 🔭 Vantage: Spec for CSRF Protection

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-03-29
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As a Security-Conscious Developer, I want built-in CSRF protection for my state-changing HTML forms, so that I can prevent cross-site request forgery attacks without writing custom middleware.

---

## The "So What?" Ask

What business problem does this solve?
Without CSRF protection, applications built with Autumn are vulnerable by default to state-changing actions executed by malicious sites. Providing built-in, easy-to-use CSRF protection ensures our users can build secure applications with confidence, improving the framework's credibility as "production-ready."

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Provides robust, automatic CSRF protection that developers can easily configure.
- **Loco / Other Rust Frameworks:** Often require manual middleware setup or third-party crates for CSRF.
- **Our Gap:** Autumn currently lacks an out-of-the-box, framework-integrated solution that pairs seamlessly with our form handlers and HTML templating, forcing users to cobble together external crates.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must generate and validate CSRF tokens for POST, PUT, and DELETE form submissions.
- Must provide a simple, declarative way to render the token as a hidden input within HTML templates.
- Must return a `403 Forbidden` status with a clear error message upon validation failure.
- Must be enabled by default for form handlers and disabled by default for JSON API handlers.
- Must allow global and route-specific configuration via the standard configuration file.

---

## Metric Definition

Success = Zero developer effort required to enable CSRF protection on a standard HTML form, and <1ms added latency per request for token validation on the 99th percentile.

---

## Out of Scope

🚫 **Out of Scope:**
- CSRF protection for Single Page Applications (SPAs) using custom headers (Phase 2).
- Advanced double-submit cookie patterns beyond the standard session-based or signed-cookie token generation.
