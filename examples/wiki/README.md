# Autumn Wiki Example

A small wiki showing how Autumn's mutation hooks and generated repositories fit
together when you need lifecycle logic, revision history, and a JSON API
without hand-writing the CRUD boilerplate twice.

## What it demonstrates

- `#[model]` for `Page` and `Revision`
- `#[repository(Page, hooks = PageHooks, api = "/api/v1/pages")]`
- Mutation hooks for slug generation and revision auditing
- Maud templates for a server-rendered editing flow
- Embedded migrations on startup
- Framework health and actuator endpoints

## Quick start

From the workspace root:

```bash
# 1. Download Tailwind CSS
cargo run -p autumn-cli -- setup

# 2. Start Postgres
docker compose -f examples/wiki/docker-compose.yml up -d

# 3. Run the app
cargo run -p wiki
```

Open <http://localhost:3000>.

## Hook behavior

`PageHooks` keeps the interesting invariants in one place:

- `before_create` slugifies the title and fills in a default `"draft"` status
- `before_update` re-slugifies when the title changes

That means the UI routes and the generated REST API both get the same lifecycle
behavior automatically.

## Routes

### HTML

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | List all pages |
| GET | `/new` | New page form |
| POST | `/pages` | Create a page |
| GET | `/pages/{slug}` | View a page |
| GET | `/pages/{slug}/edit` | Edit form |
| POST | `/pages/{slug}` | Update a page |
| GET | `/pages/{slug}/history` | Full revision history |

### JSON API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/pages` | List pages |
| GET | `/api/v1/pages/{id}` | Fetch one page |
| POST | `/api/v1/pages` | Create a page |
| PUT | `/api/v1/pages/{id}` | Update a page |
| DELETE | `/api/v1/pages/{id}` | Delete a page |

### Framework

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health probe |
| GET | `/actuator/health` | Detailed health view |
| GET | `/actuator/info` | Build and runtime metadata |
| GET | `/actuator/metrics` | Request and pool metrics |
| GET | `/static/js/htmx.min.js` | Bundled htmx |
| GET | `/static/css/autumn.css` | Compiled Tailwind output |
