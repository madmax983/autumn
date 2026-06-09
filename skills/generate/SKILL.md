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
| `scaffold` | `scaffold Post title:String body:Text` | Model + migration + controller (index/show/new/create/edit/update/delete) + views + smoke test |
| `model` | `model Post title:String body:Text` | Model struct + migration |
| `migration` | `migration add_slug_to_posts` | Empty timestamped migration file |
| `controller` | `controller Posts` | Routes file with CRUD stubs |
| `mailer` | `mailer UserMailer welcome` | Mailer struct + email templates |
| `task` | `task RecalculateCounts` | `#[task]` operational command |
| `auth` | `auth User --oauth github,google` | Full auth scaffold (login/register/password reset/OAuth) |
| `admin` | `admin Post` | Admin plugin resource page |
| `system-test` | `system-test checkout-flow` | System test fixture |

## Field type reference

| Token | SQL type | Rust type |
|---|---|---|
| `String` | `VARCHAR(255) NOT NULL` | `String` |
| `Text` | `TEXT NOT NULL` | `String` |
| `Integer` / `BigInt` | `BIGINT NOT NULL` | `i64` |
| `Float` | `FLOAT8 NOT NULL` | `f64` |
| `Boolean` | `BOOLEAN NOT NULL DEFAULT false` | `bool` |
| `DateTime` | `TIMESTAMPTZ NOT NULL DEFAULT NOW()` | `DateTime<Utc>` |
| `Uuid` | `UUID NOT NULL DEFAULT gen_random_uuid()` | `Uuid` |
| `references:Model` | `BIGINT NOT NULL REFERENCES models(id)` | `i64` (FK) |
| `name:unique` | Adds `UNIQUE` constraint | — |
| `name:index` | Adds B-tree index | — |

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
1. Register routes in main.rs:
   .routes(routes![
       routes::posts::index,
       routes::posts::show,
       routes::posts::new,
       routes::posts::create,
       routes::posts::edit,
       routes::posts::update,
       routes::posts::delete_post,
   ])
2. Run: autumn migrate
3. Run: autumn dev
```

### model
```
Next steps:
1. Add the model module to src/main.rs or src/models/mod.rs
2. Run: autumn migrate
3. Implement repository functions in src/repositories/<name>.rs
```

### migration
```
Next steps:
1. Edit the generated migration file in migrations/<timestamp>_<name>/
2. Write up.sql (and down.sql if rollback matters)
3. Run: autumn migrate
```

### controller
```
Next steps:
1. Register routes in main.rs:
   .routes(routes![routes::<name>::index, ...])
2. Add #[secured] to routes that require authentication
```

### mailer
```
Next steps:
1. Call the mailer from a job or route:
   UserMailer::welcome(&user).deliver_later().await?;
2. Preview at: http://localhost:3000/dev/mailer/previews (dev mode)
```

### task
```
Next steps:
1. Register in main.rs:
   .one_off_tasks(one_off_tasks![tasks::recalculate_counts])
2. Invoke with: autumn task recalculate_counts -- --dry-run
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
