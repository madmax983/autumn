# 🔭 Vantage: Spec for OpenAPI Auto-Generation

## 👤 User Story
As a Backend Developer, I want the framework to automatically generate an OpenAPI (Swagger) specification from my route definitions and data models, so that front-end teams and API consumers always have up-to-date documentation without me maintaining it manually.

## 💼 The "So What?" (Business Problem)
Stale API documentation is a massive source of friction between frontend and backend teams. If developers have to write docs by hand, they will drift from the actual implementation. By auto-generating OpenAPI specs directly from our `#[get]`, `#[post]`, and `#[model]` macros, Autumn eliminates this drift. This significantly reduces integration time and bugs, accelerating the overall product delivery cycle. A framework that acts as the single source of truth for both code and contract is a major selling point for team-wide adoption.

## 🎯 Success Metrics
* **Accuracy:** 100% of generated routes accurately reflect the expected request/response payloads and HTTP status codes defined in the Rust code.
* **Developer Effort:** Developers can serve a Swagger UI at `/swagger-ui` by adding a single configuration line or macro.
* **Adoption:** 30% of new REST API projects built with Autumn enable the OpenAPI generation feature.

## 🔍 Gap Analysis
* **The Market:** Spring Boot (via `springdoc-openapi`), FastAPI (built-in natively), and .NET all provide zero-configuration OpenAPI generation.
* **Our Current State:** Autumn currently requires developers to maintain API contracts manually or pull in external crates like `utoipa`, manually decorating every handler with verbose, redundant annotations that duplicate the information already present in our routing macros.
* **The Gap:** We need to parse our existing `#[get]`, `#[post]`, and extractor types at compile time (via our existing macros) to automatically generate the OpenAPI schema registry without requiring duplicate `#[utoipa::path]` annotations.

## ✅ Acceptance Criteria
* The framework must automatically infer route paths, HTTP methods, and path parameters from existing Autumn routing macros.
* The framework must inspect JSON extractors and responses to generate valid JSON schemas for the OpenAPI document.
* Must provide an endpoint (e.g., `/v3/api-docs`) that serves the generated `openapi.json` file.
* Must optionally serve a Swagger UI or Redoc interface out-of-the-box.
* Must allow developers to override or enrich the auto-generated documentation (e.g., descriptions, custom status codes) via an optional `#[api_doc(...)]` macro.

## 🚫 Out of Scope
* Client SDK generation (e.g., generating TypeScript clients from the spec). This is better handled by existing tools like `openapi-generator`.
* Support for GraphQL schemas or gRPC definitions.
