---
name: generate
description: >
  Use when the user runs /autumn:generate, asks to scaffold a resource,
  generate a model, migration, controller, mailer, or task using the
  autumn CLI.
argument-hint: "<subcommand> <Name> [fields...] [flags]"
allowed-tools:
  - Bash
  - Read
  - Write
  - Edit
---

# autumn:generate

Wrap `autumn generate <subcommand>` commands. Always show a redacted command
preview and get confirmation before executing any mutating generator.

## Supported subcommands

| Subcommand | Example | What it creates |
|---|---|---|
| `scaffold` | `scaffold Post title:String body:Text` | Model + migration + routes (index/show/new/create/edit/update/delete) + views + smoke test. Also updates `src/main.rs`. |
| `model` | `model Post title:String body:Text` | Model struct + migration |
| `migration` | `migration add_slug_to_posts` | Empty timestamped migration file |
| `mailer` | `mailer User` | Mailer struct + email templates (generator appends `Mailer` → produces `UserMailer`) |
| `task` | `task RecalculateCounts` | `#[task]` operational command |
| `auth` | `auth User --oauth github,google` | Full auth scaffold (login/register/password reset/OAuth) |
| `admin` | `admin Post title:String body:Text` | Admin plugin resource page — fields must be supplied explicitly; generator does not read the model |
| `system-test` | `system-test checkout_flow` | System test fixture (name must be `snake_case` or `PascalCase` — no hyphens) |
| `pwa` | `pwa` | PWA scaffolding — manifest, service worker, offline shell, icons, route handlers, smoke test |

## Field type reference

Use the exact tokens below — the DSL parser is case-sensitive and does not
accept aliases like `Integer` or `Boolean`.

| Token | SQL type | Rust type |
|---|---|---|
| `String` | `TEXT NOT NULL` | `String` |
| `Text` | `TEXT NOT NULL` | `String` (alias for String) |
| `i32` | `INTEGER NOT NULL` | `i32` |
| `i64` | `BIGINT NOT NULL` | `i64` |
| `f32` | `REAL NOT NULL` | `f32` |
| `f64` | `DOUBLE PRECISION NOT NULL` | `f64` |
| `bool` | `BOOLEAN NOT NULL` | `bool` |
| `NaiveDateTime` | `TIMESTAMP NOT NULL` | `NaiveDateTime` |
| `DateTime` | `TIMESTAMPTZ NOT NULL` | `DateTime<Utc>` |
| `Uuid` | `UUID NOT NULL` | `Uuid` |
| `Bytea` | `BYTEA NOT NULL` | `Vec<u8>` |
| `Attachment` | `JSONB NULL` (blob metadata) | `Option<Blob>` (always nullable) — **requires `storage` feature in Cargo.toml**; generator does not add it automatically |
| `Option<T>` | Nullable version of any above | `Option<T>` |

**Indexes and UNIQUE constraints are not field tokens.** Add them by hand in the
generated migration's `up.sql` after scaffolding.

**Foreign keys are not in the DSL.** To add an FK column (e.g. `user_id`), scaffold the
model with an `i64` field and then hand-edit the generated migration to add
`REFERENCES users(id)` and an index.

**Do not use UUID as a primary key.** Primary keys are always `i64` /
`BIGSERIAL`. Use `Uuid` as a secondary column for external correlation only.

## Execution flow

1. Parse the subcommand and arguments from user input.
2. Build the full `autumn generate` command string.
3. Show the redacted preview:
   ```
   Will run: autumn generate scaffold Post title:String body:Text
   Project root: /path/to/project
   ```
4. Ask for confirmation: "Proceed? (yes/no)"
5. Only execute after explicit confirmation.
6. Run the command and capture stdout, stderr, and exit code.
7. Show what was created (the file list from stdout).
8. Show the mandatory next steps for the subcommand (see below).

## Next steps per subcommand

### scaffold
```
Next steps:
1. Run: autumn migrate   (the generator already updated src/main.rs)
2. Run: autumn dev
3. Visit: http://localhost:3000/<plural>
```

### model
```
Next steps:
1. The generator already created src/models/<snake>.rs and added
   `pub mod <snake>;` to src/models/mod.rs — no manual wiring needed.
2. Run: autumn migrate
3. Implement repository functions (free functions or #[autumn_web::repository])
```

### migration
```
Next steps:
1. Edit the generated migration file in migrations/<timestamp>_<name>/
2. Write up.sql (and down.sql if rollback matters)
3. Run: autumn migrate
```

### mailer
```
Next steps:
1. The #[mailer] macro generates send_<method> (async) and deliver_later_<method>
   (fire-and-forget) from each fn in the impl block. Call from a handler or job:

   // The generated method name matches the snake_case of your mailer name.
   // For `autumn generate mailer User`, the method is named `user`:
   // async send (awaits delivery):
   UserMailer.send_user(&mailer, to).await?;

   // fire-and-forget (background, no await):
   UserMailer.deliver_later_user(&mailer, to);

   // Rename the method in the generated file to get send_welcome, etc.:
   // pub fn welcome(&self, to: String) -> Mail { ... }
   // → generates send_welcome / deliver_later_welcome

   Both take a &Mailer extractor as their first argument after &self.
   Add `mailer: Mailer` to the handler's extractor list to get the handle.

2. The generator already adds mod mailers; and .mail_previews(...) to main.rs.
   The type lives at mailers::<snake>::<PascalName>Mailer, e.g.:
   use mailers::user::UserMailer;
   .mail_previews(mail_previews![UserMailer])

3. Preview at: http://localhost:3000/_autumn/mail (dev mode only)
```

### task
```
Next steps:
1. The generator writes the file to tasks/<name>.rs at the project root
   (not inside src/). Move it to src/tasks/<name>.rs or src/tasks.rs,
   or reference it from src/main.rs with:
   // Generator writes tasks/<snake_name>.rs at project root.
   // In src/main.rs, reference it with a path attribute:
   #[path = "../tasks/recalculate_counts.rs"]
   mod recalculate_counts;

   // Or move the file to src/tasks/<snake_name>.rs and add:
   // mod tasks; (with src/tasks/<snake_name>.rs inside it)

2. Register in main.rs (use the snake_case module and function name):
   .one_off_tasks(one_off_tasks![recalculate_counts::recalculate_counts])

3. Invoke with: autumn task recalculate_counts -- --dry-run
```

### pwa
```
Next steps:
1. The generator already:
   - Created static/manifest.webmanifest, static/service-worker.js,
     static/pwa-register.js, static/icons/icon.svg (+ maskable variant)
   - Added route handlers (/manifest.webmanifest, /service-worker.js,
     /pwa-register.js, /offline) and PWA <meta>/<link> tags to src/main.rs
   - Created tests/system/pwa_smoke.rs and added system-tests to Cargo.toml

2. Replace static/icons/icon.svg with a real PNG icon for mobile installation.
   For iOS, also add 180×180 apple-touch-icon.png.

3. Edit static/manifest.webmanifest to set your app name, theme_color,
   background_color, and start_url.

4. Run the smoke test: cargo test --features system-tests pwa_smoke
```

## Flags

- `--api`: Generate JSON-only scaffold (no HTML views)
- `--oauth github,google`: For `auth` subcommand — add OAuth providers
- `--totp`: For `auth` — add TOTP two-factor auth
- `--passkeys`: For `auth` — add WebAuthn passkeys

## Error handling

If `autumn generate` exits nonzero, show the stderr output and diagnose:
- File conflict: offer to show the conflicting file
- Missing autumn-cli: instruct `cargo install autumn-cli --version 0.5.0`
- Schema error: check `src/schema.rs` is up to date (`autumn migrate`)
