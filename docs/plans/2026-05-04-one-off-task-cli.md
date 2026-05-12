# One-Off Task CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `#[autumn_web::task]` and `autumn task` so apps can run named one-off operational scripts with normal Autumn app context.

**Architecture:** A task attribute macro generates `OneOffTaskInfo` metadata and an async handler that resolves task parameters through the same `FromRequestParts<AppState>` extractor path used by routes. `AppBuilder` gets a separate one-off task registration list and task/list modes triggered by `autumn-cli` through environment variables, mirroring `autumn routes` static introspection.

**Tech Stack:** Rust 2024, Tokio, Axum extractors, existing Autumn config/telemetry/database/mail bootstrap, clap, serde_json, proc macros.

---

### Task 1: Runtime Task Types And Argument Parsing

**Files:**
- Modify: `autumn/src/task.rs`
- Test: `autumn/src/task.rs`

**Steps:**
1. Write tests for converting `["--user-id", "42", "--dry-run"]` into a query string.
2. Write tests for parsing that query string into `TaskArgs<T>`.
3. Implement `OneOffTaskInfo`, `OneOffTaskHandler`, `TaskArgs<T>`, and `request_parts_for_task_args`.
4. Run `cargo test -p autumn-web task::`.

### Task 2: Task Macro And Collection Macro

**Files:**
- Create: `autumn-macros/src/one_off_task.rs`
- Create: `autumn-macros/src/one_off_tasks_macro.rs`
- Modify: `autumn-macros/src/lib.rs`
- Modify: `autumn/src/lib.rs`
- Modify: `autumn/src/prelude.rs`
- Test: `autumn/tests/compile-pass/task_basic.rs`

**Steps:**
1. Write a compile-pass test proving `#[task]` captures name, description, extractors, and `TaskArgs<T>`.
2. Watch it fail because the macro and collection macro do not exist.
3. Implement `#[task]` and `one_off_tasks![]`.
4. Run the compile-pass test.

### Task 3: AppBuilder Task Mode

**Files:**
- Modify: `autumn/src/app.rs`
- Test: `autumn/src/app.rs`

**Steps:**
1. Write tests for task-list JSON and missing-task errors.
2. Add `AppBuilder::one_off_tasks`.
3. Add `AUTUMN_RUN_TASK`, `AUTUMN_LIST_TASKS`, and `AUTUMN_TASK_ARGS_JSON` modes before normal server startup.
4. Reuse config, telemetry, DB, mail, storage, i18n, policy, audit, and startup-hook boot paths where relevant.
5. Run targeted app/task tests.

### Task 4: CLI Task Command And Generator

**Files:**
- Create: `autumn-cli/src/task.rs`
- Create: `autumn-cli/src/generate/task.rs`
- Modify: `autumn-cli/src/main.rs`
- Modify: `autumn-cli/src/generate/mod.rs`
- Test: `autumn-cli/src/main.rs`
- Test: `autumn-cli/src/task.rs`
- Test: `autumn-cli/tests/generate.rs`

**Steps:**
1. Write failing parser tests for `autumn task cleanup --user-id 42`, `--list`, `--profile`, `--package`, and `--bin`.
2. Write failing generator tests for `tasks/cleanup_users.rs`.
3. Implement CLI task execution/listing through env-triggered app binary runs.
4. Implement `autumn generate task <name>`.
5. Run `cargo test -p autumn-cli`.

### Task 5: Example And Docs

**Files:**
- Modify: `examples/blog/src/main.rs`
- Create: `examples/blog/src/tasks.rs`
- Create: `docs/guide/tasks.md`
- Modify: `README.md`

**Steps:**
1. Add a discoverable `cleanup_posts` task to the blog example and register it.
2. Document declaration, registration, listing, running, argument parsing, and failure behavior.
3. Link the guide from README.
4. Run formatting, targeted tests, and affected-area stub scan.
