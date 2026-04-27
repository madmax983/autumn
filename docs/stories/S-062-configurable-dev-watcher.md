# 🔭 Vantage: Spec for Configurable Dev Watcher

**Epic:** EPIC-001 (Project Scaffolding)
**Priority:** Should Have
**Story Points:** 3
**Status:** Not Started
**Assigned To:** unassigned
**Created:** 2026-04-22
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to configure which directories the `autumn dev` server watches for changes, so that my custom folder structures (like `views/` or `assets/`) trigger hot-reloads automatically without requiring manual restarts.

---

## The "So What?" Ask

What business problem does this solve?
Developer experience and velocity are directly tied to the speed of the edit-refresh cycle. Currently, `autumn dev` hardcodes the watched directories (`src`, `static`, `templates`, `migrations`), which penalizes teams that deviate from the default structure. When changes are silently ignored, developers waste time debugging "broken" code that simply hasn't been recompiled. A configurable watcher reduces friction, prevents wasted engineering hours, and makes the framework adaptable to varying team conventions.

---

## Gap Analysis

Look at the market:
- **Node.js (Nodemon / Vite):** Allows highly configurable watch paths, ignore patterns, and extensions via command-line arguments or config files (`nodemon.json`, `vite.config.js`).
- **Cargo Watch:** The Rust standard for watching files allows specifying multiple watch paths via `-w`.
- **Our Gap:** `autumn dev` currently lacks the flexibility of standard tooling by hardcoding watched directories. We need to expose a configuration option in `autumn.toml` to let users add custom directories (e.g., `views`) to the watch list, bridging the gap between convention and configuration.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must allow developers to specify additional directories to watch via a `[dev]` section in `autumn.toml` (e.g., `watch_dirs = ["views", "locales"]`).
- Must continue to watch the default directories (`src`, `static`, `templates`, `migrations`) even if custom directories are provided.
- Must accurately detect file changes in the configured custom directories and trigger a rebuild/reload of the application.
- Must document the new configuration options and the default watched directories in the README or CLI documentation.

---

## Metric Definition

Success = A developer can add a custom `views` directory to their `autumn.toml`, modify an HTML file inside it, and see the application rebuild and reload in <1s without manual intervention.

---

## Out of Scope

🚫 **Out of Scope:**
- Granular file extension filtering (e.g., watching only `.html` files in a specific directory).
- Hot Module Replacement (HMR) for injecting changes without a full page reload or process restart.
