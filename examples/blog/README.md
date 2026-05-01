# Autumn Blog Example

A blog engine built with Autumn's full-stack path: Diesel, Maud, Tailwind,
htmx, embedded migrations, and the new hybrid-rendering pipeline.

## What it demonstrates

- Public blog listing and slug-based post pages
- Admin UI for create, edit, publish, and delete
- JSON endpoints alongside server-rendered HTML
- Validation before insert/update
- `#[static_get]` + `static_routes![]` for build-time rendering
- Automatic migrations on startup
- Framework health and actuator endpoints
- **i18n end-to-end** — opt-in `i18n` feature, `i18n/{en,es}.ftl`
  translations, the request-scoped `Locale` extractor, the `t!()` macro
  with **compile-time key validation**, automatic fallback, and a
  locale switcher in the layout (visit `/greet`)

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

## Try the i18n flow

The blog enables the `i18n` Cargo feature on `autumn-web` and ships
two translation bundles at `i18n/en.ftl` and `i18n/es.ftl`. The shared
`layout()` is fully translated through the `t!()` macro, the nav has
a built-in language switcher, and `/greet` renders an end-to-end demo:

```text
http://localhost:3000/greet              # default (Accept-Language)
http://localhost:3000/greet?locale=es    # Spanish via query override
http://localhost:3000/greet?locale=en    # English via query override
```

Resolution order is documented in [`docs/guide/i18n.md`](../../docs/guide/i18n.md):
query → signed session cookie → plain `autumn_locale` cookie →
`Accept-Language` → default. Try editing a key in `i18n/en.ftl` or
introducing a typo in a `t!()` call — the build fails with a
"did you mean" hint thanks to the proc-macro's compile-time check.

## Routes

### HTML

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Public blog listing |
| GET | `/about` | Static page rendered via `#[static_get]` |
| GET | `/greet` | i18n demo with locale switcher |
| GET | `/posts/{slug}` | View a published post |
| GET | `/admin` | Admin post dashboard |
| GET | `/admin/new` | New post form |
| POST | `/admin` | Create a post |
| GET | `/admin/{id}/edit` | Edit post form |
| POST | `/admin/{id}` | Update a post |
| DELETE | `/admin/{id}` | Delete a post with htmx |

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
| GET | `/static/js/htmx.min.js` | Bundled htmx |
| GET | `/static/css/autumn.css` | Compiled Tailwind output |
