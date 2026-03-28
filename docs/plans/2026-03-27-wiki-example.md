# Wiki Example with Mutation Hooks — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a wiki example app with full Maud + HTMX UI that showcases mutation hooks — slug auto-generation, status transition guards, and transactional revision history via after_* hooks.

**Architecture:** The wiki has two tables (pages, revisions). A `PageHooks` struct implements `MutationHooks` to auto-generate slugs in `before_create`/`before_update`, guard status transitions in `before_update`, and write audit revisions in `after_create`/`after_update`. The `after_*` hooks need a database connection to write revisions inside the same transaction — this requires adding a `conn` parameter to the `after_*` trait methods (framework change). The UI uses Maud templates with HTMX for the revision sidebar.

**Tech Stack:** Rust, Autumn framework (autumn-web, autumn-macros), diesel-async (Postgres), Maud (HTML), HTMX (interactivity), Tailwind CSS (styling)

---

## Task 1: Add `conn` parameter to `after_*` hooks (framework change)

**Files:**
- Modify: `autumn/src/hooks.rs` (after_create, after_update, after_delete signatures)
- Modify: `autumn-macros/src/repository.rs` (pass conn into after_* calls)
- Modify: `autumn/tests/hooks_lifecycle.rs` (update test)
- Modify: `autumn/tests/compile-pass/repository_with_hooks.rs` (if needed)

**Step 1: Change `after_create` signature in hooks.rs**

In `autumn/src/hooks.rs`, change:
```rust
fn after_create(
    &self,
    _ctx: &MutationContext,
    _record: &Self::Model,
) -> impl Future<Output = AutumnResult<()>> + Send {
    async { Ok(()) }
}
```
to:
```rust
fn after_create(
    &self,
    _ctx: &MutationContext,
    _record: &Self::Model,
    _conn: &mut ::diesel_async::AsyncPgConnection,
) -> impl Future<Output = AutumnResult<()>> + Send {
    async { Ok(()) }
}
```

Do the same for `after_update` and `after_delete`.

Note: `diesel_async` is already a dependency of `autumn-web` behind the `db` feature. Import `diesel_async::AsyncPgConnection` at the top of hooks.rs.

**Step 2: Update `NoHooks` and inline test**

The `NoHooks` impl uses default methods, so it inherits automatically.

Update the `no_hooks_all_methods_are_noop` test in hooks.rs — the `after_*` calls need a `&mut AsyncPgConnection`. Since we don't have a real connection in unit tests, we need to either:
- Skip the after_* calls in the unit test (they're already tested via compile-pass/db tests)
- Or use a mock

Simplest: remove the after_* assertions from the unit test (they're covered by compile-pass tests and db_hooks_lifecycle tests). Keep the test focused on before_* and after_commit.

**Step 3: Update generated repository code**

In `autumn-macros/src/repository.rs`, the `after_create`, `after_update`, and `after_delete` calls happen inside the transaction closure where `conn` is available. Change:

Save body (line ~298):
```rust
hooks.after_create(ctx_ref, &record).await?;
```
to:
```rust
hooks.after_create(ctx_ref, &record, conn).await?;
```

Update body (line ~351):
```rust
hooks.after_update(ctx_ref, &record).await?;
```
to:
```rust
hooks.after_update(ctx_ref, &record, conn).await?;
```

Delete body (similar pattern):
```rust
hooks.after_delete(ctx_ref, id).await?;
```
to:
```rust
hooks.after_delete(ctx_ref, id, conn).await?;
```

**Step 4: Update hooks_lifecycle.rs integration test**

Remove the `after_create`, `after_update`, `after_delete` assertions from the `no_hooks_methods_are_all_ok` test (they require a real connection now).

**Step 5: Update db_hooks_lifecycle.rs**

The testcontainers tests that call hooks directly will need to pass a connection to `after_*`. Update the `after_create_rejection_rolls_back` and `after_commit_failure_does_not_rollback` tests — these tests call hooks inside transactions where `conn` is available, so pass it through.

**Step 6: Run tests**

Run: `cargo check` (must compile)
Run: `cargo test -p autumn-web -p autumn-macros` (all pass)
Run: `cargo test -p autumn-web --test compile_fail` (compile tests pass)

**Step 7: Commit**

```bash
git add autumn/src/hooks.rs autumn-macros/src/repository.rs autumn/tests/
git commit -m "feat(hooks): add conn parameter to after_* hooks for transactional side-effects"
```

---

## Task 2: Wiki example scaffolding

**Files:**
- Modify: `Cargo.toml` (add wiki to workspace members)
- Create: `examples/wiki/Cargo.toml`
- Create: `examples/wiki/docker-compose.yml`
- Create: `examples/wiki/autumn.toml`
- Create: `examples/wiki/autumn-dev.toml`
- Create: `examples/wiki/build.rs`
- Create: `examples/wiki/tailwind.config.js`
- Create: `examples/wiki/static/css/input.css`
- Create: `examples/wiki/migrations/00000000000000_create_wiki/up.sql`
- Create: `examples/wiki/migrations/00000000000000_create_wiki/down.sql`

**Step 1: Add wiki to workspace**

In root `Cargo.toml`, add `"examples/wiki"` to the `members` array.

**Step 2: Create `examples/wiki/Cargo.toml`**

```toml
[package]
name = "wiki"
edition.workspace = true
version.workspace = true
publish = false

[dependencies]
autumn-web = { path = "../../autumn" }
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono"] }
diesel-async = { version = "0.8", features = ["postgres"] }
pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }
diesel_migrations = "2"
maud = { version = "0.27", features = ["axum"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
```

**Step 3: Create `examples/wiki/docker-compose.yml`**

```yaml
services:
  db:
    image: postgres:16
    environment:
      POSTGRES_DB: wiki
      POSTGRES_USER: autumn
      POSTGRES_PASSWORD: autumn
    ports:
      - "5433:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data

volumes:
  pgdata:
```

Note: port 5433 to avoid conflict with bookmarks on 5432.

**Step 4: Create config files**

`examples/wiki/autumn.toml`:
```toml
[server]
port = 3001

[log]
level = "info"

[health]
path = "/health"
```

`examples/wiki/autumn-dev.toml`:
```toml
[database]
url = "postgres://autumn:autumn@localhost:5433/wiki"
```

**Step 5: Create `examples/wiki/build.rs`**

Copy from bookmarks example (identical Tailwind build logic).

**Step 6: Create Tailwind config**

`examples/wiki/tailwind.config.js`:
```javascript
/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./src/**/*.rs"],
  theme: { extend: {} },
  plugins: [],
};
```

`examples/wiki/static/css/input.css`:
```css
@import "tailwindcss";
```

**Step 7: Create migration**

`examples/wiki/migrations/00000000000000_create_wiki/up.sql`:
```sql
CREATE TABLE pages (
    id BIGSERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    body TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'draft',
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_pages_slug ON pages (slug);
CREATE INDEX idx_pages_status ON pages (status);

CREATE TABLE revisions (
    id BIGSERIAL PRIMARY KEY,
    page_id BIGINT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    op TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    changed_by TEXT,
    summary TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_revisions_page_id ON revisions (page_id);
```

`examples/wiki/migrations/00000000000000_create_wiki/down.sql`:
```sql
DROP TABLE IF EXISTS revisions;
DROP TABLE IF EXISTS pages;
```

**Step 8: Verify scaffolding compiles**

Create a minimal `examples/wiki/src/main.rs`:
```rust
fn main() {
    println!("wiki placeholder");
}
```

Run: `cargo check -p wiki`
Expected: compiles

**Step 9: Commit**

```bash
git add Cargo.toml examples/wiki/
git commit -m "chore: scaffold wiki example with migrations and config"
```

---

## Task 3: Schema, models, slugify helper

**Files:**
- Create: `examples/wiki/src/schema.rs`
- Create: `examples/wiki/src/models.rs`
- Create: `examples/wiki/src/slugify.rs`

**Step 1: Create Diesel schema**

`examples/wiki/src/schema.rs`:
```rust
diesel::table! {
    pages (id) {
        id -> Int8,
        title -> Text,
        slug -> Text,
        body -> Text,
        status -> Text,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    revisions (id) {
        id -> Int8,
        page_id -> Int8,
        op -> Text,
        title -> Text,
        body -> Text,
        status -> Text,
        changed_by -> Nullable<Text>,
        summary -> Nullable<Text>,
        created_at -> Timestamp,
    }
}
```

**Step 2: Create models**

`examples/wiki/src/models.rs`:
```rust
use crate::schema::{pages, revisions};

#[autumn_web::model]
pub struct Page {
    #[id]
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub body: String,
    pub status: String,
    #[default]
    pub created_at: chrono::NaiveDateTime,
    #[default]
    pub updated_at: chrono::NaiveDateTime,
}

// Revision is manual — no CRUD repo needed, just insert + query
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = revisions)]
pub struct Revision {
    pub id: i64,
    pub page_id: i64,
    pub op: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub changed_by: Option<String>,
    pub summary: Option<String>,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, Clone, diesel::Insertable)]
#[diesel(table_name = revisions)]
pub struct NewRevision {
    pub page_id: i64,
    pub op: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub changed_by: Option<String>,
    pub summary: Option<String>,
}
```

**Step 3: Create slugify helper**

`examples/wiki/src/slugify.rs`:
```rust
/// Convert a title to a URL-safe slug.
///
/// "Hello World!" -> "hello-world"
/// "Rust & WebAssembly" -> "rust--webassembly"
pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_title() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn special_characters() {
        assert_eq!(slugify("Rust & WebAssembly!"), "rust-webassembly");
    }

    #[test]
    fn already_slug() {
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
    }

    #[test]
    fn leading_trailing_hyphens() {
        assert_eq!(slugify("  spaced out  "), "spaced-out");
    }
}
```

**Step 4: Verify**

Run: `cargo test -p wiki` (slugify tests pass)
Run: `cargo check -p wiki`

**Step 5: Commit**

```bash
git add examples/wiki/src/
git commit -m "feat(wiki): add schema, models, and slugify helper"
```

---

## Task 4: PageHooks implementation

**Files:**
- Create: `examples/wiki/src/hooks.rs`
- Create: `examples/wiki/src/repositories.rs`

**Step 1: Create PageHooks**

`examples/wiki/src/hooks.rs`:
```rust
use autumn_web::hooks::{MutationContext, MutationHooks, MutationOp, UpdateDraft};
use autumn_web::AutumnResult;
use diesel_async::{AsyncPgConnection, RunQueryDsl};

use crate::models::{NewPage, NewRevision, Page, PageDraftExt, UpdatePage};
use crate::schema::revisions;
use crate::slugify::slugify;

#[derive(Clone, Default)]
pub struct PageHooks;

impl MutationHooks for PageHooks {
    type Model = Page;
    type NewModel = NewPage;
    type UpdateModel = UpdatePage;

    async fn before_create(
        &self,
        _ctx: &mut MutationContext,
        new: &mut NewPage,
    ) -> AutumnResult<()> {
        // Auto-generate slug from title
        new.slug = slugify(&new.title);
        Ok(())
    }

    async fn after_create(
        &self,
        ctx: &MutationContext,
        record: &Page,
        conn: &mut AsyncPgConnection,
    ) -> AutumnResult<()> {
        // Write initial "create" revision
        let revision = NewRevision {
            page_id: record.id,
            op: "create".into(),
            title: record.title.clone(),
            body: record.body.clone(),
            status: record.status.clone(),
            changed_by: ctx.actor.clone(),
            summary: Some("created".into()),
        };
        diesel::insert_into(revisions::table)
            .values(&revision)
            .execute(conn)
            .await
            .map_err(autumn_web::AutumnError::from)?;
        Ok(())
    }

    async fn before_update(
        &self,
        _ctx: &mut MutationContext,
        draft: &mut UpdateDraft<Page>,
    ) -> AutumnResult<()> {
        // Regenerate slug when title changes
        if draft.title().changed() {
            draft.slug().set(slugify(draft.title().after()));
        }

        // Block archived -> draft transition
        if draft.status().changed_from(&"archived".to_string())
            && draft.status().changed_to(&"draft".to_string())
        {
            return Err(autumn_web::AutumnError::bad_request_msg(
                "Cannot un-archive a page",
            ));
        }

        // Always bump updated_at
        draft.updated_at().set(chrono::Utc::now().naive_utc());

        Ok(())
    }

    async fn after_update(
        &self,
        ctx: &MutationContext,
        record: &Page,
        conn: &mut AsyncPgConnection,
    ) -> AutumnResult<()> {
        // Build change summary from draft (we only know the final state here)
        let revision = NewRevision {
            page_id: record.id,
            op: "update".into(),
            title: record.title.clone(),
            body: record.body.clone(),
            status: record.status.clone(),
            changed_by: ctx.actor.clone(),
            summary: Some("updated".into()),
        };
        diesel::insert_into(revisions::table)
            .values(&revision)
            .execute(conn)
            .await
            .map_err(autumn_web::AutumnError::from)?;
        Ok(())
    }
}
```

**Step 2: Create repository**

`examples/wiki/src/repositories.rs`:
```rust
use crate::hooks::PageHooks;
use crate::models::Page;
use crate::schema::pages;

#[autumn_web::repository(Page, hooks = PageHooks)]
pub trait PageRepository {
    fn find_by_slug(slug: String) -> Vec<Page>;
    fn find_by_status(status: String) -> Vec<Page>;
}
```

**Step 3: Verify compilation**

Update `examples/wiki/src/main.rs` to declare modules:
```rust
mod hooks;
mod models;
mod repositories;
mod schema;
mod slugify;

fn main() {
    println!("wiki placeholder");
}
```

Run: `cargo check -p wiki`

**Step 4: Commit**

```bash
git add examples/wiki/src/
git commit -m "feat(wiki): add PageHooks with slug gen, status guard, and revision audit"
```

---

## Task 5: Routes — page CRUD with Maud + HTMX

**Files:**
- Create: `examples/wiki/src/routes/mod.rs`
- Create: `examples/wiki/src/routes/pages.rs`
- Create: `examples/wiki/src/routes/revisions.rs`

**Step 1: Create routes/mod.rs**

```rust
pub mod pages;
pub mod revisions;
```

**Step 2: Create page routes**

`examples/wiki/src/routes/pages.rs`:

This file implements the full CRUD with Maud templates. Key routes:

- `GET /` — page list with status badges
- `GET /pages/new` — create form
- `POST /pages` — create (form submit → redirect)
- `GET /pages/:slug` — page view with revision sidebar (HTMX)
- `GET /pages/:slug/edit` — edit form (title, body, status dropdown)
- `POST /pages/:slug` — update (form submit → redirect)
- `DELETE /pages/:slug` — archive via HTMX

The implementer should follow the bookmarks example patterns:
- `layout()` function with nav, Tailwind classes
- `page_card()` component for list items
- `status_badge()` helper for draft/published/archived
- Form patterns matching bookmarks `new_form` and `create`
- HTMX delete with `hx-delete`, `hx-target`, `hx-confirm`
- Page view shows body content on the left, revision sidebar on the right loaded via `hx-get`

Use `PgPageRepository` (extracted from request state) for all DB operations. Use `PageRepository` trait for method signatures. Look up pages by slug using `find_by_slug()`.

For the update route: accept form data, build an `UpdatePage` with `Patch::Set` for changed fields and `Patch::Unchanged` for others. The hooks handle slug regen and revision writing.

**Step 3: Create revision sidebar route**

`examples/wiki/src/routes/revisions.rs`:

- `GET /pages/:slug/revisions` — returns an HTMX partial (just the revision list, no layout wrapper)

Query revisions by page_id, ordered by created_at DESC. Render each as a small card with timestamp, op badge, and summary.

This route uses raw Diesel queries (not a repository) since `Revision` doesn't have its own repo.

**Step 4: Verify compilation**

Run: `cargo check -p wiki`

**Step 5: Commit**

```bash
git add examples/wiki/src/routes/
git commit -m "feat(wiki): add Maud + HTMX routes for pages and revision sidebar"
```

---

## Task 6: Wire up main.rs and verify end-to-end

**Files:**
- Modify: `examples/wiki/src/main.rs`

**Step 1: Implement main.rs**

```rust
// Wiki — an Autumn example showcasing mutation hooks:
//
//   Slug generation   → before_create / before_update auto-generate slug from title
//   Status guards     → before_update blocks archived → draft transitions
//   Revision history  → after_create / after_update write audit trail in same transaction
//   Draft accessors   → draft.title().changed(), draft.status().changed_to()
//
// Run with:  docker compose -f examples/wiki/docker-compose.yml up -d
//            cargo run -p wiki

mod hooks;
mod models;
mod repositories;
mod routes;
mod schema;
mod slugify;

use autumn_web::prelude::*;
use diesel::Connection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    let config = autumn_web::config::AutumnConfig::load().expect("load config");
    if let Some(url) = &config.database.url {
        let mut conn =
            diesel::PgConnection::establish(url).expect("connect to database for migrations");
        conn.run_pending_migrations(MIGRATIONS)
            .expect("run migrations");
    }

    autumn_web::app()
        .routes(routes![
            routes::pages::list,
            routes::pages::new_form,
            routes::pages::create,
            routes::pages::view,
            routes::pages::edit_form,
            routes::pages::update,
            routes::pages::archive,
            routes::revisions::sidebar,
        ])
        .run()
        .await;
}
```

**Step 2: Verify full build**

Run: `cargo check -p wiki`
Run: `cargo clippy -p wiki --all-targets`

**Step 3: Commit**

```bash
git add examples/wiki/src/main.rs
git commit -m "feat(wiki): wire up main.rs with migrations and routes"
```

---

## Task 7: Test end-to-end with Docker

**Step 1: Start database**

```bash
cd examples/wiki && docker compose up -d
```

**Step 2: Run the app**

```bash
cargo run -p wiki
```

**Step 3: Manual verification**

1. Visit http://localhost:3001 — empty page list
2. Click "New Page" — create form
3. Submit a page with title "Getting Started" — verify slug auto-generated, redirects to list
4. Click the page — view with revision sidebar showing "created" entry
5. Click "Edit" — edit form with current values
6. Change title to "Getting Started Guide" — submit, verify slug updated, new revision appears
7. Change status to "archived" — submit, verify badge updates
8. Try changing status back to "draft" — verify error (blocked by hook)

**Step 4: Fix any issues found during manual testing**

**Step 5: Final commit**

```bash
git add -A
git commit -m "fix(wiki): polish from end-to-end testing"
git push
```
