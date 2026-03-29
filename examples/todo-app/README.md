# Autumn Todo App

A full-stack reference application demonstrating the classic Autumn stack:

- **Diesel + diesel-async** for Postgres database access
- **Maud** templates with **Tailwind CSS** styling
- **htmx** for interactive toggle and delete without page reloads
- **JSON API** alongside server-rendered HTML
- **AutumnError** for consistent error handling (404, 422, 500)
- **Embedded migrations** so the schema comes up with the app
- **Framework ops endpoints** via `/health` and `/actuator/*`

## Prerequisites

- Rust (edition 2024)
- Docker & Docker Compose (for Postgres)

## Quick start

From the **workspace root** (`autumn/`):

```bash
# 1. Download Tailwind CSS CLI
cargo run -p autumn-cli -- setup

# 2. Start Postgres
docker compose -f examples/todo-app/docker-compose.yml up -d

# 3. Run the application
cargo run -p todo-app
```

The server starts at <http://localhost:3000>.

## Available routes

### HTML (browser)

| Method | Path                  | Description                    |
|--------|-----------------------|--------------------------------|
| GET    | `/`                   | Redirect to `/todos`           |
| GET    | `/todos`              | List all todos                 |
| GET    | `/todos/{id}`         | Todo detail page               |
| POST   | `/todos`              | Create todo (form submission)  |
| POST   | `/todos/{id}/toggle`  | Toggle completion (htmx)       |
| DELETE | `/todos/{id}`         | Delete todo (htmx)             |

### JSON API

| Method | Path          | Description            |
|--------|---------------|------------------------|
| GET    | `/api/todos`  | List all todos (JSON)  |
| POST   | `/api/todos`  | Create todo (JSON)     |

### Framework

| Method | Path                      | Description              |
|--------|---------------------------|--------------------------|
| GET    | `/health`                 | Health check             |
| GET    | `/actuator/health`        | Detailed health view     |
| GET    | `/actuator/info`          | Build and runtime info   |
| GET    | `/actuator/metrics`       | Request and pool metrics |
| GET    | `/static/js/htmx.min.js` | Bundled htmx             |
| GET    | `/static/css/autumn.css`  | Compiled Tailwind CSS    |
