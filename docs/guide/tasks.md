# One-Off Tasks

Use one-off tasks for operational scripts that need normal Autumn context but
should not become HTTP routes: backfills, cleanup jobs, customer-specific data
fixes, replay scripts, and reports.

Tasks run in the app binary through `autumn task <name>`. The framework loads
the active profile, initializes tracing, creates the configured database pool,
installs mail/config extractors, runs startup hooks, executes the task, then
runs shutdown hooks.

## Declare A Task

```rust
use autumn_web::config::AutumnConfig;
use autumn_web::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CleanupArgs {
    #[serde(default)]
    pub confirm: bool,
}

/// Clean up unpublished draft posts.
#[autumn_web::task(name = "cleanup-posts")]
pub async fn cleanup_posts(
    mut db: Db,
    config: AutumnConfig,
    TaskArgs(args): TaskArgs<CleanupArgs>,
) -> AutumnResult<()> {
    if !args.confirm {
        autumn_web::reexports::tracing::info!(
            profile = ?config.profile,
            "dry run; pass --confirm to apply changes"
        );
        return Ok(());
    }

    // Use `db` exactly as you would in a route handler.
    Ok(())
}
```

Register it on the app builder:

```rust
mod tasks;

use autumn_web::prelude::*;

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .one_off_tasks(one_off_tasks![tasks::cleanup_posts])
        .run()
        .await;
}
```

## Run Tasks

List registered tasks:

```bash
autumn task --list
```

Run a task with the active profile:

```bash
autumn task cleanup-posts --confirm
autumn task --profile prod cleanup-posts --confirm
```

Workspace apps can select a package and binary:

```bash
autumn task --package blog --bin blog --list
autumn task --package blog cleanup-posts --confirm
```

Task arguments use long-flag syntax. Flags become snake_case fields on
`TaskArgs<T>`:

```bash
autumn task backfill-user --user-id 42 --dry-run
```

deserializes as:

```rust
#[derive(serde::Deserialize)]
struct BackfillArgs {
    user_id: i64,
    dry_run: bool,
}
```

A task returning `Err(AutumnError)` exits non-zero and prints the error to
stderr, so CI and cron can rely on normal process status.

## Generate A Skeleton

```bash
autumn generate task cleanup_users
```

This creates `tasks/cleanup_users.rs` with a `#[task]` skeleton and adds
`serde` if the project does not already depend on it. To register a generated
root-level task file from `src/main.rs`:

```rust
#[path = "../tasks/cleanup_users.rs"]
mod cleanup_users_task;

autumn_web::app()
    .routes(routes![index])
    .one_off_tasks(one_off_tasks![cleanup_users_task::cleanup_users]);
```

For larger apps, keeping task modules under `src/tasks/` and using normal Rust
module declarations is often cleaner. The runtime only requires that the task
function is registered with `.one_off_tasks(...)`.
