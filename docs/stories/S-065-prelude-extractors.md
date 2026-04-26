# 🔭 Vantage: Spec for Prelude Extractor Ergonomics

**Epic:** EPIC-002 (Developer Experience Polish)
**Priority:** Must Have
**Story Points:** 2
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-26
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want common extractors like `Path` and `Query` to be included in the `autumn_web::prelude`, so that I don't have to write long, deep import statements just to read basic request parameters in my route handlers.

---

## The "So What?" Ask

What business problem does this solve?
Currently, developers building simple endpoints are forced to memorize or look up deep module paths (like `autumn_web::extract::Path<String>`) just to extract variables from the URL. This increases the cognitive load, litters code with boilerplate imports, and contradicts Autumn's core value proposition of providing an ergonomic, "Spring Boot-style" experience out of the box. By pulling these common tools into the prelude, we reduce friction for new users and keep example code clean and readable.

---

## Gap Analysis

Look at the market:
- **Axum (Underlying Framework):** Provides standard extractors, which must be imported manually or glob-imported by users.
- **Actix Web:** Includes `web::Path`, `web::Query`, `web::Json`, etc., cleanly under a unified module, often imported as `use actix_web::web;`.
- **Our Gap:** The `autumn_web::prelude` currently only exports `Json` and `Form`, leaving `Path` and `Query` out. We are forcing users to type a 4-level deep import path for the most basic web routing features.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- The `Path` extractor must be exported in `autumn_web::prelude`.
- The `Query` extractor must be exported in `autumn_web::prelude`.
- Existing documentation (like the `examples/hello` app and the `README.md`) must be updated to use the shorter `Path` type provided by the prelude instead of the fully qualified path.
- Existing applications using the full path must not be broken (it should be an additive re-export).

---

## Metric Definition

Success = A user can write `async fn hello_name(name: Path<String>)` with only `use autumn_web::prelude::*;` at the top of the file, and the code compiles without any "unresolved import" errors.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatically injecting `Path` and `Query` into the global namespace without a prelude import.
- Re-architecting the Axum extraction layer itself.
