# Autumn Blog Example

A blog engine built with Autumn's full-stack path: Diesel, Maud, Tailwind,
htmx, embedded migrations, and the new hybrid-rendering pipeline.

## What it demonstrates

- Public blog listing and slug-based post pages
- Admin UI for create, edit, publish, and delete
- First-party `autumn-admin-plugin` mounted at `/backoffice`
- JSON endpoints alongside server-rendered HTML
- Validation before insert/update
- `#[static_get]` + `static_routes![]` for build-time rendering
- Automatic migrations on startup
- Framework health and actuator endpoints

## Quick start

From the workspace root:

```bash
# 1. Download Tailwind CSS
cargo run -p autumn-cli -- setup

# 2. Start Postgres
docker compose -f examples/blog/docker-compose.yml up -d

# 3. Run the app (migrations are embedded and applied on startup)
cargo run -p blog
```

Open <http://localhost:3000>.

## Try the hybrid-rendering flow

The `/about` page is declared with `#[static_get("/about")]`.

```bash
# Pre-render static routes into dist/
cargo run -p autumn-cli -- build -p blog
```

Dynamic routes like `/` and `/posts/{slug}` still render through the server;
static routes get written to `dist/` for deployment or CDN serving.

## Fingerprinted assets (production cache-busting)

Autumn ships a zero-config asset fingerprinting pipeline.  When you run a
release build the CLI hashes every file under `static/`, writes a
content-hashed copy alongside the original, and records the mapping in
`static/.autumn-manifest.json`:

```
static/css/autumn.css            ← original (kept for dev)
static/css/autumn.a1b2c3d4.css  ← fingerprinted copy (served in production)
static/.autumn-manifest.json    ← logical → fingerprinted path map
```

### End-to-end production deploy story

```bash
# 1. Build a release binary and fingerprint all static assets in one step
cargo run -p autumn-cli -- build -p blog

# 2. Start the server (reads static/.autumn-manifest.json at runtime)
./target/release/blog
```

Templates call `asset_url("css/autumn.css")` instead of a hard-coded path:

```rust
// In src/routes/posts.rs — unchanged across environments
link rel="stylesheet" href=(asset_url("css/autumn.css"));
```

| Build mode | Resolved URL |
|-----------|--------------|
| `cargo run` (debug) | `/static/css/autumn.css` |
| `cargo build --release` | `/static/css/autumn.a1b2c3d4.css` |

The fingerprinted URL is served with `Cache-Control: public, max-age=31536000,
immutable`, so browsers cache it forever.  On the next deploy the hash changes
automatically — the old URL is gone, the browser fetches the new one, and no
CDN or cache purge is needed.

Non-fingerprinted static paths (`/static/css/autumn.css`) are served with
`Cache-Control: public, max-age=0, must-revalidate` so development and fallback
paths always stay fresh.

### How `asset_url` works

```rust
use autumn_web::prelude::*;   // asset_url is in the prelude

// debug build  → "/static/css/autumn.css"
// release build → "/static/css/autumn.a1b2c3d4.css"
let url = asset_url("css/autumn.css");
```

If `static/.autumn-manifest.json` is absent (e.g. on a dev machine that never
ran `autumn build --release`), `asset_url` falls back to the plain
`/static/...` URL — so the app keeps running without any manual configuration.

## Routes

### HTML

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Public blog listing |
| GET | `/about` | Static page rendered via `#[static_get]` |
| GET | `/posts/{slug}` | View a published post |
| GET | `/admin` | Admin post dashboard |
| GET | `/admin/new` | New post form |
| POST | `/admin` | Create a post |
| GET | `/admin/{id}/edit` | Edit post form |
| POST | `/admin/{id}` | Update a post |
| DELETE | `/admin/{id}` | Delete a post with htmx |
| GET | `/backoffice/posts` | Admin plugin list view for posts |

### JSON API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/posts` | List published posts as JSON |
| POST | `/api/posts` | Create a post as JSON |

### Framework

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health probe |
| GET | `/actuator/health` | Detailed health view |
| GET | `/actuator/info` | Build and runtime metadata |
| GET | `/actuator/metrics` | Request and pool metrics |
| GET | `/static/js/htmx.min.js` | Bundled htmx (plain) or `/static/js/htmx.min.<hash>.js` (release) |
| GET | `/static/css/autumn.css` | Compiled Tailwind output (plain) or `/static/css/autumn.<hash>.css` (release) |
