# API Tokens Example

Demonstrates first-class API bearer token authentication with a persistent
Postgres-backed store and `RequireApiToken` middleware.

## What it demonstrates

| Feature | Where | What it does |
|---------|-------|--------------|
| **`DbApiTokenStore`** | `main.rs` | Persists tokens hashed at rest in Postgres |
| **`API_TOKEN_MIGRATIONS`** | `main.rs` | Auto-creates the `api_tokens` table |
| **`RequireApiToken`** | `main.rs` | Rejects requests with missing/invalid tokens |
| **`ApiToken` extractor** | `whoami`, `revoke_current` handlers | Injects the verified principal ID |
| **`DeferredStore`** | `main.rs` | Bridges middleware construction and pool availability |
| **`autumn token issue`** | CLI | Issues tokens from the terminal without running the app |
| **`autumn token revoke`** | CLI | Revokes tokens from the terminal |

## Routes

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/tokens/:principal_id` | None | Issue a new bearer token |
| `GET` | `/me` | Bearer token | Return the authenticated principal |
| `DELETE` | `/tokens/current` | Bearer token | Revoke the presented token |

## Prerequisites

- Rust (edition 2024)
- Docker & Docker Compose (for Postgres)
- `autumn-cli` binary on PATH

## Quick start

From the **workspace root** (`autumn/`):

```bash
# 1. Start Postgres
docker compose -f examples/api-tokens/docker-compose.yml up -d

# 2. Run the application (migrations run automatically in dev)
cargo run -p api-tokens
```

The server starts at <http://localhost:3000>.

## Try it with curl

### Issue a token

```bash
TOKEN=$(curl -s -X POST http://localhost:3000/tokens/user:42)
echo "Token: $TOKEN"
```

The raw token is returned as plain text. **It is shown only once** — store it
in a secrets manager or environment variable.

### Call a protected endpoint

```bash
curl -s http://localhost:3000/me \
  -H "Authorization: Bearer $TOKEN"
# → authenticated as user:42
```

Requests without a valid `Authorization: Bearer <token>` header receive
`401 Unauthorized`.

### Revoke a token

```bash
curl -s -X DELETE http://localhost:3000/tokens/current \
  -H "Authorization: Bearer $TOKEN"
# → 204 No Content

# Subsequent requests with the same token are rejected:
curl -s http://localhost:3000/me \
  -H "Authorization: Bearer $TOKEN"
# → 401 Unauthorized
```

## Managing tokens from the CLI

The `autumn token` subcommand provides terminal access to the same operations
without requiring the application to be running:

```bash
# Issue a token for a service account
TOKEN=$(autumn token issue service:my-worker)
echo "Service token: $TOKEN"

# Revoke it later
autumn token revoke "$TOKEN"
```

The CLI reads the database URL from `autumn.toml` or the `DATABASE_URL` /
`AUTUMN_DATABASE__URL` environment variables, and calls `psql` to execute the
SQL directly — no running app required.

## Configuration (`autumn.toml`)

```toml
[database]
url = "postgres://localhost/api_tokens_dev"
```

Or set `DATABASE_URL` in the environment:

```bash
export DATABASE_URL="postgres://localhost/api_tokens_dev"
```
