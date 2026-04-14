# 🔭 Vantage: Spec for OpenTelemetry Integration

**Epic:** EPIC-012 (v1.1 Observability)
**Priority:** Must Have
**Story Points:** 8
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-02
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Operator, I want built-in OpenTelemetry integration so that I can trace requests across microservices and export metrics to APM platforms (like Prometheus/Jaeger/Datadog) without writing custom tower middleware.

---

## The "So What?" Ask

**What business problem does this solve?**
When Autumn applications reach production, operators need to understand performance bottlenecks and trace errors across service boundaries. Currently, developers must manually wire up `tracing-opentelemetry`, `opentelemetry-otlp`, and custom Axum layers. This boilerplate is tedious, easy to get wrong, and distracts from shipping product features. By providing declarative OpenTelemetry support, we make Autumn a first-class citizen in enterprise environments and reduce the Mean Time To Resolution (MTTR) for production incidents.

---

## Gap Analysis

**Look at the market:**
- **Spring Boot:** Offers Actuator and Micrometer, which seamlessly integrate with OpenTelemetry, Prometheus, and countless APMs via simple `application.properties` configuration.
- **Loco / Other Rust Frameworks:** Often rely on the community to piece together `tracing` and `opentelemetry` crates, lacking a unified, configuration-driven approach.
- **Our Gap:** Autumn has structured logging (`tracing`) and basic `/actuator/metrics`, but no distributed tracing context propagation or OTLP export. Developers have to drop out of the "Autumn way" to get enterprise observability.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must allow configuring an OTLP endpoint (e.g., `grpc://localhost:4317`) via `autumn.toml`.
- Must automatically extract W3C Trace Context from incoming HTTP requests and inject it into outgoing HTTP responses.
- Must propagate trace context to spawned background tasks (`#[scheduled]`).
- Must automatically instrument database queries (`diesel-async`) with spans when the `db` feature is enabled.
- Must fall back to standard `tracing-subscriber` logging if OTLP export fails or is not configured.

---

## Metric Definition

Success = An operator can enable distributed tracing to Jaeger simply by setting `AUTUMN_TELEMETRY__OTLP_ENDPOINT=http://jaeger:4317` in their deployment environment, with zero Rust code changes required. Request latency overhead from tracing must be <2ms per request.

---

## Out of Scope

🚫 **Out of Scope:**
- Custom metric pipelines beyond standard HTTP request duration/count (custom application metrics are Phase 2).
- Supporting proprietary tracing protocols (e.g., direct Datadog agent protocols, though OTLP will work for them).
- Log export via OTLP (focusing only on traces and metrics for Phase 1).
