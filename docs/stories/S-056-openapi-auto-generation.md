# 🔭 Vantage: Spec for OpenAPI Auto-Generation

**Epic:** EPIC-013 (v1.2 Developer Experience)
**Priority:** Must Have
**Story Points:** 8
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-12
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As a Backend Developer, I want the framework to automatically generate an OpenAPI (Swagger) specification from my route definitions and data models, so that front-end teams and API consumers always have up-to-date documentation without me maintaining it manually.

---

## The "So What?" Ask

**What business problem does this solve?**
Writing OpenAPI specs by hand is tedious, error-prone, and almost always drifts from the actual implementation. Outdated documentation leads to friction between frontend and backend teams, broken API integrations, and increased support costs for external consumers. By auto-generating the OpenAPI spec directly from the Rust source code, we guarantee that the documentation is always accurate and synchronized with the deployed code. This reduces developer overhead, accelerates frontend development, and improves the overall developer experience of consumers using the API.

---

## Gap Analysis

**Look at the market:**
- **FastAPI / Spring Boot / NestJS:** These frameworks have set the industry standard by providing rich, auto-generated OpenAPI documentation out of the box based on type signatures and decorators.
- **Utoipa / Salvo (Rust):** `utoipa` provides macros to generate OpenAPI docs, but often requires significant manual annotation. Frameworks like Salvo integrate OpenAPI generation more tightly but are outside our ecosystem.
- **Our Gap:** Autumn currently requires developers to manually maintain an `openapi.yaml` file alongside their code, or piece together third-party crates like `utoipa` which require heavy and redundant annotations on every struct and route. There is no native, low-friction way to derive the specification directly from Autumn's existing routing macros and JSON extractors.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- The framework must automatically infer route paths, HTTP methods, and path parameters from existing Autumn routing macros.
- The framework must inspect JSON extractors and responses to generate valid JSON schemas for the OpenAPI document.
- Must provide an endpoint (e.g., `/v3/api-docs`) that serves the generated `openapi.json` file.
- Must optionally serve a Swagger UI or Redoc interface out-of-the-box.
- Must allow developers to override or enrich the auto-generated documentation via an optional `#[api_doc(...)]` macro.

---

## Metric Definition

Success = A developer can generate a complete OpenAPI v3 spec and view it in Swagger UI simply by enabling the `openapi` feature in `autumn.toml`, with zero additional code annotations required for basic CRUD routes.

---

## Out of Scope

🚫 **Out of Scope:**
- Client SDK generation.
- Support for GraphQL schemas or gRPC definitions.
