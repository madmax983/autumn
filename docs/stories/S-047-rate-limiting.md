# 🔭 Vantage: Spec for Rate Limiting

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-03-31
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Operator, I want built-in rate limiting capabilities for my endpoints, so that I can protect my service from abuse, accidental denial of service attacks, and excessive cost from high-volume traffic.

---

## The "So What?" Ask

What business problem does this solve?
Without rate limiting, public-facing applications built with Autumn are vulnerable to abuse, scraping, and brute-force attacks. Providing an integrated rate limiting mechanism enables developers to safeguard application availability and ensure fair usage of resources across all users without requiring external proxy configuration or complex third-party crate integration. This bolsters the framework's standing for production readiness.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Offers flexible rate limiting options often implemented via Spring Cloud Gateway or Bucket4j.
- **Loco / Other Rust Frameworks:** Often depend on developers hand-rolling middleware or pulling in crates like `governor` with manual state management.
- **Our Gap:** Autumn currently exposes endpoints with unlimited throughput by default. We lack a zero-configuration, framework-native way to throttle requests per IP or per user, placing the burden of infrastructure protection entirely on the developer.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must provide a declarative way to apply rate limits globally and to specific route handlers (e.g., via an attribute macro or configuration).
- Must return a `429 Too Many Requests` status code when a limit is exceeded, complete with a `Retry-After` HTTP header.
- Must support configuration via the standard `autumn.toml` file (e.g., requests per second, burst capacity).
- Must default to an in-memory token bucket or sliding window algorithm for standalone deployments.
- Must accurately isolate limits based on client IP address or an extracted identity.

---

## Metric Definition

Success = Zero developer effort required to enable a global rate limit via configuration, and <2ms added latency per request for rate limit evaluation at the 99th percentile.

---

## Out of Scope

🚫 **Out of Scope:**
- Distributed rate limiting across multiple server instances using Redis or Memcached (Phase 2).
- Dynamic rate limit adjustment based on real-time server load or complex billing tiers.
