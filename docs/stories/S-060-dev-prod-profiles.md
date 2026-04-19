# 🔭 Vantage: Spec for Dev/Prod Profiles

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-19
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to easily separate my configuration between local development and production environments without maintaining entirely separate files, so that I can have convenient defaults locally (like pretty logging and permissive CORS) while automatically locking down settings when deployed.

---

## The "So What?" Ask

What business problem does this solve?
Configuration errors between development and production are a leading cause of security breaches and operational failures. By providing built-in, convention-based profile management, we reduce the cognitive load on developers and eliminate the "it worked on my machine" class of errors. This standardizes how Autumn applications transition from local prototyping to cloud-native production, directly supporting the framework's goal of being "production-ready" out of the box.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Offers highly mature `application-{profile}.properties` or `.yml` files, making profile switching trivial via `spring.profiles.active`.
- **Node.js (Express/NestJS):** Heavily relies on `NODE_ENV` and external packages like `dotenv` or `config` to merge environment-specific files.
- **Our Gap:** Currently, Autumn configurations are monolithic or overly reliant on individual environment variables. There is no unified mechanism to switch a "bundle" of settings (like database URLs, logging formats, and security policies) based on the target environment, forcing users to build their own bespoke config loading logic.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must allow `[profile.dev]` and `[profile.prod]` sections within the standard `autumn.toml` configuration file.
- Must determine the active profile via the `AUTUMN_ENV` environment variable, defaulting to `dev` if unset, with values mapping directly to the profile keys (`dev` -> `[profile.dev]`, `prod` -> `[profile.prod]`).
- Must ensure that settings in `[profile.dev]` or `[profile.prod]` override base settings when their respective profile is active.
- Must automatically switch framework-level defaults based on the active profile (e.g., structured JSON logging in production vs. pretty logging in development).
- Must ensure that explicit environment variables (e.g., `AUTUMN_DATABASE__URL`) continue to take precedence over both base and profile-specific settings.

---

## Metric Definition

Success = 0 configuration files needed beyond `autumn.toml` to support basic dev/prod separation, and the application successfully boots with correct profile-specific settings in <50ms.

---

## Out of Scope

🚫 **Out of Scope:**
- Supporting arbitrary, user-defined profiles beyond `dev` and `prod` (e.g., `staging`, `testing`) for the initial v1.0 release.
- Dynamic reloading of configuration changes at runtime without restarting the process.
- Integration with external configuration management systems (e.g., AWS Parameter Store, HashiCorp Vault).
