# Autumn Web Harvest Rename Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rename the Autumn adapter crate from `autumn-harvest-autumn` to `autumn-web-harvest` and update all local consumers, docs, and verification targets.

**Architecture:** Keep `autumn-harvest` as the standalone workflow engine workspace and keep the Autumn adapter as a thin integration crate. This change is a crate-identity refactor only: rename the package, update import paths and workspace references, and preserve runtime behavior.

**Tech Stack:** Rust 2024, Cargo workspaces, Tokio, Diesel, Autumn web framework.

---

### Task 1: Prove the new crate identity is not wired yet

**Files:**
- Modify: `examples/reddit-clone/Cargo.toml`
- Test: `examples/reddit-clone/src/main.rs`

**Step 1: Write the failing compile-facing change**

Change the example to depend on and import `autumn-web-harvest`.

**Step 2: Run verification to confirm failure**

Run: `cargo check -p reddit-clone`

Expected: FAIL because the renamed crate does not exist yet.

### Task 2: Rename the adapter crate and local references

**Files:**
- Modify: `autumn-harvest/Cargo.toml`
- Modify: `autumn-harvest/autumn-web-harvest/Cargo.toml`
- Modify: `autumn-harvest/autumn-harvest/src/builder.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/prelude.rs`
- Modify: `autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs`
- Modify: `autumn-harvest/CLAUDE.md`
- Modify: `examples/reddit-clone/Cargo.toml`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `docs/plans/2026-04-05-harvest-core-adapter-split.md`

**Step 1: Rename the package**

Update the adapter crate package name from `autumn-harvest-autumn` to `autumn-web-harvest`.

**Step 2: Update all code references**

Replace `autumn_harvest_autumn` imports and doc examples with `autumn_web_harvest`.

**Step 3: Update doc references**

Replace prose references to the old crate name where they describe the current architecture.

### Task 3: Verify both workspaces still build

**Files:**
- No code changes

**Step 1: Verify Harvest workspace**

Run: `cargo test -j 1 --workspace`
Workdir: `autumn-harvest`

Expected: PASS

**Step 2: Verify the example wiring from the parent workspace**

Run: `cargo check -p reddit-clone`

Expected: PASS
