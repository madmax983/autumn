# 🔭 Vantage: Spec for Secure Headers

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 3
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-04
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Operator and Developer, I want security-related HTTP headers set by default on all responses, so that I can protect my application from common web vulnerabilities (like clickjacking and MIME-type sniffing) without having to manually configure proxy headers or custom middleware.

---

## The "So What?" Ask

What business problem does this solve?
Default security headers are a baseline expectation for any production-ready framework. Without them, applications are needlessly exposed to vulnerabilities that are trivial to mitigate. By providing these out of the box, we protect our users by default, saving them from failing security audits and reducing the risk of exploits. This increases the trust and credibility of the Autumn framework.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Provides a comprehensive set of default security headers (e.g., `X-Content-Type-Options`, `X-XSS-Protection`, `Cache-Control`, `X-Frame-Options`, `Strict-Transport-Security`).
- **Loco / Other Rust Frameworks:** Often require users to add tower-http's `SetResponseHeaderLayer` manually with their own configuration, which means developers must know which headers to set.
- **Our Gap:** Autumn currently doesn't enforce these security headers by default, leaving the application vulnerable unless the developer is a security expert and manually sets them up.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must apply the following HTTP headers to all outgoing responses by default:
  - `X-Content-Type-Options: nosniff`
  - `X-Frame-Options: DENY` (or `SAMEORIGIN`)
  - `Referrer-Policy: strict-origin-when-cross-origin`
- Must include `Strict-Transport-Security` (HSTS) when running in the `production` profile.
- Must provide a straightforward way to override or disable these headers via `autumn.toml` (e.g., if the user needs to allow embedding the app in an iframe).
- Must seamlessly integrate with the existing application middleware stack.

---

## Metric Definition

Success = Zero developer effort to achieve a passing grade on basic security header scans (e.g., Mozilla Observatory), with <1ms overhead added per request.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic generation of a dynamic `Content-Security-Policy` (CSP) based on script/style hashes (Phase 2).
- Feature-Policy / Permissions-Policy headers which require deep application-specific knowledge.
