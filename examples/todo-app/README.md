# Autumn Todo App

A full-stack reference application demonstrating Autumn's key features:

- **Diesel + diesel-async** for Postgres database access
- **Maud** templates with **Tailwind CSS** styling
- **htmx** for interactive toggle and delete without page reloads
- **JSON API** alongside server-rendered HTML
- **AutumnError** for consistent error handling (404, 422, 500)

## Prerequisites

- Rust (edition 2024)
- Docker & Docker Compose (for Postgres)
- Tailwind CSS CLI — run `autumn setup` in this directory, or install manually

## Quick start

```bash
# 1. Start Postgres
docker compose up -d

# 2. Run the application
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
| GET    | `/static/js/htmx.min.js` | Bundled htmx             |
| GET    | `/static/css/autumn.css`  | Compiled Tailwind CSS    |
