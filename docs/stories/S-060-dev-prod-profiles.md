# 🔭 Vantage: Spec for Dev/Prod Profiles

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 8
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-18
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to define environment-specific configurations via `dev` and `prod` profiles, so that my application can seamlessly transition from local development (with pretty logging and relaxed constraints) to production (with structured logging and strict security) without code changes.

---

## The "So What?" Ask

What business problem does this solve?
Configuration divergence between development and production is a major source of deployment failures and operational incidents. If a framework requires developers to manually wire up conditional logic for logging formats, database URLs, and server binds across environments, mistakes will happen. By providing built-in, convention-based `dev` and `prod` profiles with "smart defaults," Autumn ensures applications are secure and observable in production by default, while remaining ergonomic and developer-friendly on local machines.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Pioneered the `@Profile` and `application-{profile}.yml` approach, making environment configuration effortless and standard.
- **Loco / Other Rust Frameworks:** Often rely purely on `.env` files or manual `config-rs` setups, shifting the burden of defining safe production defaults onto the user.
- **Our Gap:** Autumn currently has a solid configuration system (`autumn.toml` + env vars), but lacks a framework-native understanding of "environments." We need first-class profile support to layer configuration files safely and inject secure production defaults.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must support loading environment-specific configuration files (e.g., `autumn-dev.toml`, `autumn-prod.toml`) that merge with the base `autumn.toml`.
- Must provide "smart defaults" based on the active profile (e.g., `pretty` logging in `dev`, `json` logging in `prod`).
- Must automatically detect the profile based on the build type (debug build = `dev`, release build = `prod`) unless overridden.
- Must allow overriding the active profile via an environment variable (e.g., `AUTUMN_PROFILE=staging`).
- Must expose the active profile name through the application state for conditional logic if necessary.

---

## Metric Definition

Success = Deploying an Autumn application with `--release` automatically configures JSON structured logging and strict shutdown timeouts without requiring the developer to write custom configuration logic, and profile resolution adds <1ms to application startup time.

---

## Out of Scope

🚫 **Out of Scope:**
- Supporting multiple active profiles simultaneously (Phase 2).
- Dynamic profile reloading without restarting the application.
- Encrypted configuration values or secrets management integration.
