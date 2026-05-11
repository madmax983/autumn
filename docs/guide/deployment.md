# Deploying an Autumn App

This guide walks you from a fresh `autumn new` project to a production-shaped
container running against a real Postgres database. Every command is verbatim;
no file editing is required to reach a running container.

Target time: **under 10 minutes** on a machine with Docker and a working
internet connection.

---

## Prerequisites

- **Rust 1.88.0+** with `cargo`
- **Docker** (or Docker Desktop) — `docker --version`
- **PostgreSQL** accessible at a connection string you control (local or remote)
- The `autumn` CLI - `cargo install autumn-cli --version 0.3.0`

---

## Step 1 — Create the project

```bash
autumn new myapp
cd myapp
```

This scaffolds a working Autumn application with a dev-oriented `Dockerfile`.

---

## Step 2 — Generate production deployment files

```bash
autumn release init --force
```

`--force` is required because `autumn new` already wrote a basic `Dockerfile`
and `.dockerignore`. The `--force` flag replaces them with the production-grade
versions.

The command emits three files at the project root:

| File | Purpose |
|---|---|
| `Dockerfile` | Multi-stage image: cargo-chef dep cache → release binary → debian-slim runtime |
| `.dockerignore` | Keeps `target/`, `.git/`, `node_modules/`, `dist/` out of the build context |
| `autumn.production.toml.example` | Production config template with placeholder values — no real secrets |

> **What changed in the Dockerfile?**
> The production Dockerfile adds cargo-chef dependency caching (so rebuilds only
> recompile what changed), installs `libpq`, `tini`, and `ca-certificates` in the
> slim runtime, copies compiled Tailwind assets from `static/`, leaves
> migrations to an explicit primary-role job, and wires the `/health` endpoint as the container
> `HEALTHCHECK`.

---

## Step 3 — Build the image

```bash
docker build -t myapp .
```

The first build downloads Rust crates and the Tailwind binary; subsequent builds
are fast because cargo-chef caches the dependency layer separately from your
application code.

Expected final output:

```
[...]
 => CACHED [runtime 2/7] RUN apt-get update ...
 => [runtime 7/7] COPY --from=builder /app/autumn.production.toml.example /app/autumn.toml
 => exporting to image
Successfully built <sha>
Successfully tagged myapp:latest
```

---

## Step 4 — Migrate, Then Run the Container

Provide your primary/write Postgres connection string as
`AUTUMN_DATABASE__PRIMARY_URL`. Run migrations once against that primary role
before starting web replicas:

```bash
AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod" autumn migrate
```

Then start the web container:

```bash
docker run --rm \
  -p 3000:3000 \
  -e AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod" \
  myapp
```

You should see something like:

```
INFO autumn: Listening addr=0.0.0.0:3000
```

Visit [http://localhost:3000/health](http://localhost:3000/health) — a healthy
response looks like:

```json
{ "status": "ok", "version": "0.3.0" }
```

> **Migration failure stops the rollout.** If the primary URL is wrong or the
> database is unreachable, `autumn migrate` exits non-zero and you do not roll
> the web tier. Fix the connection string and rerun the one-shot job.

---

## How the production image works

```
rust:1.88-bookworm (chef stage)
  └─ cargo chef prepare          # snapshot dependency graph
       └─ cargo chef cook        # build all dependencies (cached layer)
            └─ cargo build --release
                 └─ debian:bookworm-slim (runtime stage)
                       libpq5, tini, ca-certificates, curl
                       /usr/local/bin/myapp     ← your binary
                       /app/static/             ← compiled Tailwind + assets
                       /app/migrations/         ← SQL migration files
                       /app/autumn.toml         ← production config (host=0.0.0.0)

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/myapp"]
```

Key design decisions:

- **cargo-chef** separates the dependency build layer from your code. Changing a
  handler reuses cached dependencies; only your crate recompiles.
- **tini** is the PID 1 init process. It reaps zombie processes and forwards
  signals (SIGTERM, SIGINT) so the server shuts down gracefully.
- **Explicit migration ownership** -- migrations run once through
  `AUTUMN_DATABASE__PRIMARY_URL=... autumn migrate` before web replicas roll.
  The web image starts only the server, so replicas do not race schema changes.
- **`autumn.production.toml.example` is copied as `/app/autumn.toml`** so the
  binary binds to `0.0.0.0` (all interfaces) instead of the dev default
  `127.0.0.1`. Override any value at runtime via `AUTUMN_*` environment
  variables (see the [config reference](getting-started.md#configuration)).

---

## Customising the production config

`autumn.production.toml.example` is the starting point for production config.
It is already used by the container (copied as `/app/autumn.toml` at build time).

To change log format, pool size, or health path, edit
`autumn.production.toml.example` before building:

```toml
# autumn.production.toml.example (excerpt)
[server]
host = "0.0.0.0"
port = 3000

[log]
level = "info"
format = "Json"        # structured JSON for log aggregators

[database]
primary_url = "postgres://user:CHANGE_ME@localhost:5432/myapp_prod"
# replica_url = "postgres://user:CHANGE_ME@replica:5432/myapp_prod"
pool_size = 10
replica_fallback = "fail_readiness"
auto_migrate_in_production = false
```

Sensitive values (database password, SMTP credentials) should **never** be in
this file. Pass them as environment variables at runtime:

```bash
-e AUTUMN_DATABASE__PRIMARY_URL="postgres://user:realpass@host:5432/myapp_prod"
-e AUTUMN_LOG__LEVEL=debug
```

`AUTUMN_*` environment variables override `autumn.toml` at the highest
priority layer — see the
[config reference](getting-started.md#environment-variable-overrides).

---

## Deploy to fly.io

Scaffold a `fly.toml` alongside the production Dockerfile:

```bash
autumn release init --force --target fly
```

This creates `fly.toml` wired to the same `Dockerfile` and `/health` check.

Deploy:

```bash
fly launch --no-deploy          # creates the app on fly.io
fly secrets set AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod"
AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod" autumn migrate
fly deploy
```

Run the migration step once before `fly deploy`. If you add a read replica,
configure `AUTUMN_DATABASE__REPLICA_URL` separately and gate readiness until the
replica has replayed the latest Diesel migration.

---

## Run locally with Docker Compose (app + Postgres)

Scaffold a `docker-compose.yml` with an app service, a one-shot migration job,
and a managed Postgres:

```bash
autumn release init --force --target docker-compose
```

Start both services:

```bash
docker compose up --build
```

The `docker-compose.yml` sets `AUTUMN_DATABASE__PRIMARY_URL` pointing at the
`db` service, waits for Postgres to pass its healthcheck, runs `autumn migrate`
once, and starts the app only after that job exits successfully. No manual
Postgres setup is needed.

To reset the database:

```bash
docker compose down -v   # removes the postgres_data volume
docker compose up --build
```

---

## Overwriting specific files

By default `autumn release init` refuses to overwrite existing files:

```
Error: 'Dockerfile' already exists — run with --force to overwrite
```

Use `--force` to regenerate everything, or delete individual files first if you
only want to regenerate a subset.

---

## Signing secret (required before production boot)

Before the server will bind in the `prod` profile, you must set a stable signing
secret. It protects sessions, CSRF tokens, and signed storage URLs:

```bash
# Generate once, store securely (e.g. Fly secrets, AWS Secrets Manager, …)
export AUTUMN_SECURITY__SIGNING_SECRET="$(openssl rand -hex 32)"
```

**Smoke-gate check** — the app must refuse to boot _without_ the secret:

```bash
docker run --rm \
  -e AUTUMN_ENV=prod \
  -e AUTUMN_DATABASE__PRIMARY_URL=... \
  myapp 2>&1 | grep -i "signing secret"
# Expected: "Invalid signing secret configuration: signing secret is required in production"
```

And must start successfully _with_ a valid secret:

```bash
docker run --rm -p 3000:3000 \
  -e AUTUMN_ENV=prod \
  -e AUTUMN_DATABASE__PRIMARY_URL=... \
  -e AUTUMN_SECURITY__SIGNING_SECRET="$AUTUMN_SECURITY__SIGNING_SECRET" \
  myapp
```

See [docs/guide/signing-secrets.md](signing-secrets.md) for rotation instructions
and the full multi-replica setup guide.

---

## Multi-replica setup

To run multiple replicas behind a load balancer, every replica **must use the
same signing secret and the same Redis session backend**. A session established
on replica A must be readable by replica B.

```bash
SECRET=$(openssl rand -hex 32)

# Replica 1
docker run --rm -p 3000:3000 \
  -e AUTUMN_ENV=prod \
  -e AUTUMN_DATABASE__PRIMARY_URL=postgres://... \
  -e AUTUMN_SECURITY__SIGNING_SECRET="$SECRET" \
  -e AUTUMN_SESSION__BACKEND=redis \
  -e AUTUMN_SESSION__REDIS__URL=redis://redis:6379 \
  myapp &

# Replica 2 — identical secret, primary URL, and Redis URL
docker run --rm -p 3001:3000 \
  -e AUTUMN_ENV=prod \
  -e AUTUMN_DATABASE__PRIMARY_URL=postgres://... \
  -e AUTUMN_SECURITY__SIGNING_SECRET="$SECRET" \
  -e AUTUMN_SESSION__BACKEND=redis \
  -e AUTUMN_SESSION__REDIS__URL=redis://redis:6379 \
  myapp &
```

With this setup:

- A user who logs in via replica 1 is authenticated on replica 2 without
  re-logging in (sessions live in Redis, signed with the shared secret).
- Signed blob URLs generated on replica 1 are served correctly by replica 2
  (same HMAC key).
- CSRF tokens validate regardless of which replica handles the form submission.

---

## Next steps

Once the container is running:

- **Monitor**: `autumn monitor --url http://your-host:3000` for a live TUI
  dashboard of metrics, logs, and routes.
- **Scale**: add `min_machines_running = 1` in `fly.toml` to keep a warm
  instance; use `pool_size` in `autumn.production.toml.example` to tune
  database concurrency.
- **Observe**: uncomment the `[telemetry]` block in `autumn.production.toml.example`
  and point it at an OTLP collector for distributed tracing.
- **Harden**: run `autumn doctor --strict` in CI before building the image to
  catch config issues before they reach production.

For a full cloud-native deployment (Kubernetes readiness probes, structured
logging, OTLP tracing), see the [Cloud-Native Guide](cloud-native.md).
