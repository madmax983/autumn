# Reddit Clone

A Reddit clone built with [Autumn](https://github.com/madmax983/autumn),
showcasing the framework's major features in a single cohesive application.

## Features Demonstrated

| Feature | Where |
|---------|-------|
| Route macros (`#[get]`, `#[post]`, `#[delete]`, `routes![]`) | All route files |
| `#[autumn_web::main]` entry point | `main.rs` |
| Hybrid rendering (`#[static_get]`, `static_routes![]`) | `routes/about.rs` |
| Configuration profiles (`autumn.toml` + `autumn-dev.toml`) | Project root |
| Database (Diesel async Postgres, `Db` extractor) | All route handlers |
| Embedded migrations | `main.rs`, `migrations/` |
| `#[model]` macro with `#[id]`, `#[indexed]`, `#[validate]`, `#[default]` | `models.rs` |
| `#[repository]` with derived queries and REST API generation | `repositories.rs` |
| Mutation hooks (`before_create`, `before_update`) | `hooks.rs` |
| Session cookies (`Session` extractor, `rotate_id`, `destroy`) | `routes/auth.rs` |
| Password hashing (`hash_password`, `verify_password`) | `routes/auth.rs` |
| `#[secured]` route protection | `routes/subreddits.rs`, `routes/posts.rs` |
| CSRF protection (`CsrfToken` + htmx header injection) | `routes/layout.rs`, all forms |
| Field validation (`#[validate(length(min, max))]`) | `models.rs` |
| Scheduled background tasks (`#[scheduled(every = "15m")]`) | `tasks.rs` |
| **WebSockets** (`#[ws]`, `Channels`, durable app-db relay, pluggable live-event bus, `CancellationToken`, relay health JSON) | `routes/live.rs`, `live_events.rs`, `live_bus.rs` |
| **Durable Workflows** (real autumn-harvest onboarding + post-publication flows + management API) | `workflows.rs`, `/api/harvest/*` |
| Actuator endpoints (`/health`, `/actuator/*`) | Auto-mounted |
| Maud HTML templates | All route files |
| htmx interactivity (voting, deletion, logout) | `routes/votes.rs`, `routes/layout.rs` |
| Tailwind CSS styling | All templates |
| Static asset serving (`/static/css/`, `/static/js/htmx.min.js`) | Auto-mounted |

## Running

```bash
# Start PostgreSQL + Redis
# Fresh volumes are recommended if you want the split example too, because the
# container init script creates the separate `reddit_harvest` database once.
docker compose up -d

# Run the app in dev mode
# The first boot applies the reddit-clone schema, the framework-owned Harvest
# workflow outbox on the app database, and the Harvest storage tables on the
# configured Harvest database role.
cargo run -p reddit-clone

# Optional: watch mode from the workspace root
# cargo run -p autumn-cli -- dev -p reddit-clone

# Visit http://localhost:3000
```

If Harvest logs errors like `relation "harvest_task_queue" does not exist`, the
configured Harvest storage database never received its migrations. Start the
example once in dev mode against that database before using a release or
profile-specific run.

If app writes succeed but workflow publication stays pending, inspect the
framework-owned `harvest_workflow_outbox` table in the app database. Delivery
from that table into Harvest storage is intentionally at-least-once and depends
on idempotent workflow start.

The live-feed relay also exposes an operator-facing JSON snapshot:

```bash
curl http://localhost:3000/api/live/relay/health
```

## External Runner Escape Hatch

`reddit-clone` stays the embedded happy path, but the Harvest topology seam is
real now. If the app grows large enough to want a dedicated Harvest cluster and
separate runtime ownership, the web process can keep the API and outbox while a
different process owns worker/scheduler execution.

For a local two-process demo, use the checked-in split profiles:

```bash
# Reset Postgres so the init script creates both `reddit` and `reddit_harvest`
docker compose down -v
docker compose up -d

# Web process: app DB + Harvest API/outbox, but no local worker/scheduler
AUTUMN_PROFILE=split-web cargo run -p reddit-clone

# Separate process: owns Harvest worker/scheduler against the harvest DB
AUTUMN_PROFILE=split-runner cargo run -p reddit-clone --bin reddit-clone-harvest-runner
```

Those profiles live in:

- `autumn-split-web.toml`
- `autumn-split-runner.toml`

The runner binary reuses the same workflow/activity registration as the web app;
it just changes runtime ownership. For a true external deployment, keep the
same binary and commands, but point `harvest.database.url` at a separate
cluster instead of the local `reddit_harvest` logical database.

The live WebSocket feed keeps the app database as durable truth via
`live_feed_events`, but the wakeup path is now pluggable:

- Default/embedded mode uses Postgres `LISTEN/NOTIFY`
- Split profiles use Redis pub/sub for cross-process wakeups
- Split mode also keeps Postgres `NOTIFY` as a safety-net, so missed Redis
  publishes still wake web nodes immediately from the durable event log
- Polling is now the last fallback only when neither wake path is available

That means the database still owns replay correctness while Redis takes over
the “please wake up now” job once you outgrow letting Postgres moonlight as a
bus.

## Live Feed Operations

`/api/live/relay/health` reports the current relay state for the local process.
The important fields are:

- `listener_state`: which wake path is currently active (`postgres`, `redis`, `redis+postgres`, or `polling`)
- `reconnect_attempts` / `reconnect_successes` / `reconnect_failures`: whether the process is healing broken listeners or just taking notes about them
- `wake_redis`, `wake_postgres`, `wake_poll`: which path is actually waking the relay
- `replayed_events`, `last_seen_id`, `last_replayed_at`: whether durable rows are still flowing through replay
- `last_error`: the last relay or publish error seen by this process

Operator heuristics:

- Sustained growth in `wake_poll` means the process is living on fallback instead of a real bus.
- Growing `reconnect_failures` with a flat `reconnect_successes` means the configured wake path is still broken.
- A stale `last_replayed_at` while app writes continue means live updates are stuck before rebroadcast.
- In split mode, `listener_state = "redis+postgres"` is healthy: Redis is primary and Postgres is the backup wake path.

## WebSocket Live Feed

Connect to the live activity feed for real-time notifications:

```bash
# Global feed (all activity)
websocat ws://localhost:3000/ws/feed

# Subreddit-specific feed
websocat ws://localhost:3000/ws/r/rustlang
```

## API Endpoints

The `#[repository]` macro auto-generates read-only REST endpoints:

```bash
# Subreddits
curl http://localhost:3000/api/subreddits
curl http://localhost:3000/api/subreddits/1

# Posts
curl http://localhost:3000/api/posts
curl http://localhost:3000/api/posts/1
```

## Architecture

```
src/
  main.rs           # App builder, route + task + WS registration, migrations
  models.rs         # #[model] structs: User, Subreddit, Post, Comment, Vote
  schema.rs         # Diesel table definitions
  repositories.rs   # #[repository] with derived queries and API generation
  hooks.rs          # MutationHooks for post lifecycle (auto-slug)
  live_bus.rs       # Live-feed bus config and backend selection
  live_events.rs    # Durable app-db live-feed relay with Postgres/Redis wakeups
  tasks.rs          # Scheduled hot-rank + live-feed retention jobs
  workflows.rs      # real autumn-harvest onboarding + post-publication workflows and activities
  slugify.rs        # URL slug generation utility
  routes/
    mod.rs          # Module exports
    layout.rs       # Shared layout, vote controls, CSRF injection, time formatting
    auth.rs         # Register, login, logout, user profiles
    subreddits.rs   # Community listing, creation (#[secured]), detail view
    posts.rs        # Front page, submit, view, edit, delete
    comments.rs     # Comment creation and lazy loading
    votes.rs        # htmx-powered upvote/downvote with toggle + ON CONFLICT
    live.rs         # #[ws] WebSocket feeds consuming process-local Channels
    about.rs        # #[static_get] pre-rendered about page
```
