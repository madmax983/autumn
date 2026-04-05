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
| **WebSockets** (`#[ws]`, `Channels` pub/sub, `CancellationToken`) | `routes/live.rs` |
| **Durable Workflows** (real autumn-harvest onboarding + post-publication flows + management API) | `workflows.rs`, `/api/harvest/*` |
| Actuator endpoints (`/health`, `/actuator/*`) | Auto-mounted |
| Maud HTML templates | All route files |
| htmx interactivity (voting, deletion, logout) | `routes/votes.rs`, `routes/layout.rs` |
| Tailwind CSS styling | All templates |
| Static asset serving (`/static/css/`, `/static/js/htmx.min.js`) | Auto-mounted |

## Running

```bash
# Start PostgreSQL
docker compose up -d

# Run the app
cargo run -p reddit-clone

# Visit http://localhost:3000
```

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
  tasks.rs          # #[scheduled] hot-rank recalculator
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
    live.rs         # #[ws] WebSocket feeds with Channels pub/sub
    about.rs        # #[static_get] pre-rendered about page
```
