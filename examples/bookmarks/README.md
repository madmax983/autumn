# Autumn Bookmarks Example

A bookmark manager showcasing **Autumn v0.2 features** — profiles, validation,
the `#[model]` / `#[repository]` macros, scheduled tasks, and actuator endpoints.

## v0.2 Features Demonstrated

| Feature | Where | What it does |
|---------|-------|--------------|
| **Profiles** | `autumn.toml` + `autumn-dev.toml` | Dev profile auto-detected; DB URL only in dev config |
| **Validation** | `routes/api.rs` | `Valid<Json<NewBookmark>>` checks URL format + title length |
| **`#[model]`** | `models.rs` | Generates `Bookmark`, `NewBookmark`, `UpdateBookmark` from one struct |
| **`#[repository]`** | `repositories.rs` | Generates `PgBookmarkRepository` with CRUD + `find_by_tag` |
| **Scheduled tasks** | `tasks.rs` | `#[scheduled(every = "1h")]` link health checker |
| **Actuator** | Nav bar links | `/actuator/health`, `/actuator/info` auto-mounted |

## Prerequisites

- Rust (edition 2024)
- Docker & Docker Compose (for Postgres)

## Quick start

From the **workspace root** (`autumn/`):

```bash
# 1. Download Tailwind CSS CLI
cargo run -p autumn-cli -- setup

# 2. Start Postgres
docker compose -f examples/bookmarks/docker-compose.yml up -d

# 3. Run the application (dev profile auto-detected)
cargo run -p bookmarks
```

The server starts at <http://localhost:3000>.

## Available routes

### HTML (browser)

| Method | Path         | Description                  |
|--------|--------------|------------------------------|
| GET    | `/`          | List all bookmarks           |
| GET    | `/tag/{tag}` | Filter bookmarks by tag      |
| GET    | `/new`       | Add bookmark form            |

### JSON API

| Method | Path                   | Description                           |
|--------|------------------------|---------------------------------------|
| GET    | `/api/bookmarks`       | List all bookmarks (JSON)             |
| POST   | `/api/bookmarks`       | Create bookmark (validated JSON body) |
| DELETE | `/api/bookmarks/{id}`  | Delete bookmark                       |

### Framework

| Method | Path                     | Description            |
|--------|--------------------------|------------------------|
| GET    | `/actuator/health`       | Health + profile info  |
| GET    | `/actuator/info`         | Build & runtime info   |
| GET    | `/health`                | Health check           |
| GET    | `/static/js/htmx.min.js` | Bundled htmx          |
| GET    | `/static/css/autumn.css` | Compiled Tailwind CSS  |

## Try the validation

```bash
# Valid request
curl -X POST http://localhost:3000/api/bookmarks \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://rust-lang.org","title":"Rust","tag":"lang"}'

# Invalid URL → 422 with field errors
curl -X POST http://localhost:3000/api/bookmarks \
  -H 'Content-Type: application/json' \
  -d '{"url":"not-a-url","title":"","tag":"bad"}'
```
