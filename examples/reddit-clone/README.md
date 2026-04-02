# Reddit Clone

A Reddit clone built with [Autumn](https://github.com/madmax983/autumn),
showcasing **every major framework feature** in a single cohesive application.

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
| Mutation hooks (`before_create`, `after_create`, `before_update`) | `hooks.rs` |
| Session cookies (`Session` extractor, `rotate_id`, `destroy`) | `routes/auth.rs` |
| Password hashing (`hash_password`, `verify_password`) | `routes/auth.rs` |
| `#[secured]` route protection | `routes/subreddits.rs`, `routes/posts.rs` |
| CSRF protection (`CsrfToken` extractor + hidden form fields) | All form routes |
| Field validation (`#[validate(length(min, max))]`) | `models.rs` |
| Scheduled background tasks (`#[scheduled(every = "15m")]`) | `tasks.rs` |
| Actuator endpoints (`/health`, `/actuator/*`) | Auto-mounted |
| Maud HTML templates | All route files |
| htmx interactivity (voting, deletion) | `routes/votes.rs`, `routes/posts.rs` |
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

## API Endpoints

The `#[repository]` macro auto-generates REST endpoints:

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
  main.rs           # App builder, route + task registration, migrations
  models.rs         # #[model] structs: User, Subreddit, Post, Comment, Vote
  schema.rs         # Diesel table definitions
  repositories.rs   # #[repository] with derived queries and API generation
  hooks.rs          # MutationHooks for post lifecycle (auto-slug, logging)
  tasks.rs          # #[scheduled] hot-rank recalculator
  slugify.rs        # URL slug generation utility
  routes/
    mod.rs          # Module exports
    layout.rs       # Shared layout, vote controls, time formatting
    auth.rs         # Register, login, logout, user profiles
    subreddits.rs   # Community listing, creation (#[secured]), detail view
    posts.rs        # Front page, submit, view, edit, delete
    comments.rs     # Comment creation and lazy loading
    votes.rs        # htmx-powered upvote/downvote with toggle
    about.rs        # #[static_get] pre-rendered about page
```
