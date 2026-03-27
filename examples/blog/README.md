# Autumn Blog Example

A full-featured blog engine built with the Autumn web framework, showcasing the
complete stack: **Diesel** (Postgres), **Maud** templates, **Tailwind CSS**
styling, and **htmx** interactivity.

## Features

- **Public blog** — browse published posts with a clean, responsive UI
- **Admin dashboard** — create, edit, publish, and delete posts
- **Slug-based URLs** — auto-generated from post titles (`/posts/my-first-post`)
- **htmx delete** — inline post deletion without full page reloads
- **JSON API** — `GET /api/posts` and `POST /api/posts` for programmatic access
- **Draft/published workflow** — toggle visibility with a checkbox

## Quick start

```bash
# 1. Start Postgres
docker compose up -d

# 2. Run the app (migrations run automatically on startup)
cargo run -p blog

# 3. Open your browser
open http://localhost:3000
```

## Routes

| Method   | Path               | Description                |
|----------|--------------------|----------------------------|
| `GET`    | `/`                | Public blog listing        |
| `GET`    | `/posts/{slug}`    | View a published post      |
| `GET`    | `/admin`           | Admin post dashboard       |
| `GET`    | `/admin/new`       | New post form              |
| `POST`   | `/admin`           | Create a post              |
| `GET`    | `/admin/{id}/edit` | Edit post form             |
| `POST`   | `/admin/{id}`      | Update a post              |
| `DELETE` | `/admin/{id}`      | Delete a post (htmx)      |
| `GET`    | `/api/posts`       | JSON: list published posts |
| `POST`   | `/api/posts`       | JSON: create a post        |
| `GET`    | `/health`          | Health check               |
