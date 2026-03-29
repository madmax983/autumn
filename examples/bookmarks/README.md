# Autumn Bookmarks Example

A bookmark manager showcasing the newer Autumn feature set: profile-aware
configuration, `#[model]`, `#[repository(api = ...)]`, scheduled tasks,
embedded migrations, and actuator endpoints.

## What it demonstrates

| Feature | Where | What it does |
|---------|-------|--------------|
| **Profiles** | `autumn.toml` + `autumn-dev.toml` | Dev profile auto-detected; DB URL only in dev config |
| **`#[model]`** | `models.rs` | Generates `Bookmark`, `NewBookmark`, `UpdateBookmark` from one struct |
| **`#[repository]`** | `repositories.rs` | Generates `PgBookmarkRepository` with CRUD + `find_by_tag` + REST handlers |
| **Scheduled tasks** | `tasks.rs` | `#[scheduled(every = "1h")]` link health checker |
| **Embedded migrations** | `main.rs` | Runs Diesel migrations at startup |
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

These routes are generated from `#[autumn_web::repository(Bookmark, api = "/api/bookmarks")]`.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/bookmarks` | List all bookmarks |
| GET | `/api/bookmarks/{id}` | Fetch one bookmark |
| POST | `/api/bookmarks` | Create a bookmark |
| PUT | `/api/bookmarks/{id}` | Update a bookmark |
| DELETE | `/api/bookmarks/{id}` | Delete a bookmark |

### Framework

| Method | Path                     | Description            |
|--------|--------------------------|------------------------|
| GET    | `/actuator/health`       | Health + profile info  |
| GET    | `/actuator/info`         | Build & runtime info   |
| GET    | `/actuator/metrics`      | Request and pool stats |
| GET    | `/health`                | Health check           |
| GET    | `/static/js/htmx.min.js` | Bundled htmx          |
| GET    | `/static/css/autumn.css` | Compiled Tailwind CSS  |

## Try the generated CRUD API

```bash
# Create
curl -X POST http://localhost:3000/api/bookmarks \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://rust-lang.org","title":"Rust","tag":"lang","alive":true}'

# List
curl http://localhost:3000/api/bookmarks

# Update
curl -X PUT http://localhost:3000/api/bookmarks/1 \
  -H 'Content-Type: application/json' \
  -d '{"title":"Rust Lang","tag":"rust","alive":true}'
```
