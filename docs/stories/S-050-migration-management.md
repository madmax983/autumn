# 🔭 Vantage: Spec for Migration Management

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 5
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-06
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want database migrations to be automatically applied in development but strictly controlled in production, so that I can iterate quickly locally while avoiding accidental schema changes that might cause downtime for active users.

---

## The "So What?" Ask

What business problem does this solve?
Managing database schema evolution across environments is a major source of friction and production incidents. Developers want rapid iteration on their local machines without running explicit commands, while operations teams demand strict, predictable schema updates in production. By integrating profile-aware migration management, we eliminate the operational burden in development and provide the necessary safeguards for production, ensuring Autumn applications can scale safely.

---

## Gap Analysis

Look at the market:
- **Spring Boot:** Auto-configures tools like Flyway or Liquibase, running migrations automatically on startup depending on the environment context.
- **Loco / Other Rust Frameworks:** Often require developers to manually invoke CLI commands to apply migrations, slowing down local development loops.
- **Our Gap:** Autumn currently relies entirely on manual `diesel migration run` commands. There is no automated framework integration to streamline this process based on the deployment profile (dev vs. prod).

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must run pending Diesel migrations automatically on server startup when the application is running in the `development` profile.
- Must *not* auto-run migrations when the application is running in the `production` profile, unless explicitly opted-in via a configuration flag.
- Must provide an `autumn migrate` CLI command to explicitly run migrations in any environment.
- Must gracefully handle migration failures, logging clear error messages containing the failing SQL.
- Must log the migration status (e.g., number of migrations applied or pending) at startup.

---

## Metric Definition

Success = Zero manual CLI commands required to keep a local developer's database schema up-to-date after checking out new code, while completely preventing unauthorized auto-migrations in production.

---

## Out of Scope

🚫 **Out of Scope:**
- Automatic rollback of failed migrations (Diesel limitation; Phase 2).
- Zero-downtime migration orchestration (e.g., automatically applying view overlays).
