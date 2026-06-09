---
name: dev
description: >
  Use when the user runs /autumn:dev, asks to start the Autumn development
  server, enable hot reload, or check what's running locally.
argument-hint: "[--port <N>] [--profile <name>]"
allowed-tools:
  - Bash
  - Read
---

# autumn:dev

Start the Autumn development server with hot reload.

## Pre-flight checks

Before running `autumn dev`, verify:

1. **Database is reachable** — check `autumn.toml` or `AUTUMN_DATABASE__PRIMARY_URL`:
   ```bash
   autumn migrate check
   ```
   If this fails, the database is not available or migrations are pending.
   Pending migrations are a warning, not a blocker for `autumn dev`.

2. **Tailwind binary is present** — if `autumn setup` has not been run:
   ```bash
   autumn setup
   ```

## Execution

```bash
autumn dev
```

With optional flags:
```bash
autumn dev --port 8080
autumn dev --profile staging
```

`autumn dev` uses the `dev` profile automatically in debug builds
(`AUTUMN_ENV=dev` is the default).

## What gets served

Once running, tell the user what's available:

| Endpoint | Purpose |
|---|---|
| `http://localhost:3000` | Application root |
| `http://localhost:3000/health` | Simple health check |
| `http://localhost:3000/actuator/health` | Detailed health (JSON) |
| `http://localhost:3000/actuator/tasks` | Scheduled task status |
| `http://localhost:3000/actuator/jobs` | Background job status |
| `http://localhost:3000/static/js/htmx.min.js` | Bundled htmx |

If `autumn-admin-plugin` is installed:
| `http://localhost:3000/admin` | Admin dashboard |

If the `mail` feature is enabled:
| `http://localhost:3000/dev/mailer/previews` | Mailer preview |

## Hot reload behavior

`autumn dev` watches `src/`, `templates/`, and `static/` for changes and
recompiles + restarts automatically. Tailwind CSS is rebuilt on template
changes.

## Common failures

| Symptom | Fix |
|---|---|
| `Error: Address already in use` | Port 3000 is taken. Use `--port 3001` or kill the existing process. |
| `Error: connection refused` | Database is not running. Start Postgres first. |
| Compile error shown in terminal | Fix the Rust error; `autumn dev` will retry on next save. |
| `autumn setup` not found | Run `cargo install autumn-cli --version 0.5.0` |

## Stopping

`Ctrl+C` stops the server and the Tailwind watcher.
