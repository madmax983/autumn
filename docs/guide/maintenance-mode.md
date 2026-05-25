# Maintenance Mode

Maintenance mode lets you take an Autumn application offline in a controlled,
reversible way — without stopping the process or rolling a deploy. It is the
right tool for:

- **Destructive migrations** — schema changes that need write traffic paused
  while the migration runs.
- **Incident response** — stopping user traffic instantly while you investigate
  a data-integrity issue.
- **Planned downtime windows** — a database failover, storage volume swap, or
  dependency upgrade that requires the app to stop accepting writes briefly.

Target time: **under 30 seconds** to enter or exit maintenance on a running app.

---

## How it works

`autumn maintenance on` writes a JSON flag file at `tmp/autumn-maintenance.json`
relative to the current working directory. The running app polls that file
every 500 ms through a background task. When the file appears, every replica
enters maintenance within one poll interval — no process restart, no deploy.
`autumn maintenance off` deletes the file and the app re-opens to traffic within
the same window.

The flag file is intentionally a plain file on disk, not an in-process config
update. This means the CLI (`autumn maintenance`) runs as a **separate process**
alongside the app and needs no IPC or HTTP endpoint to communicate the state
change. Any replica that can see the same working directory (local dev, a
Docker-compose mount, a Fly.io shared volume) reacts in lock-step.

When maintenance is active, all gated requests receive **503 Service Unavailable**
with `Retry-After: 120`. The app never returns 200 for application routes while
the flag is present.

---

## Quick reference

```bash
# Enter maintenance
autumn maintenance on

# Enter maintenance with a user-visible message
autumn maintenance on --message "Down for scheduled maintenance. Back in 10 minutes."

# Exit maintenance
autumn maintenance off

# Check current status (also surfaced by `autumn doctor`)
autumn doctor
```

---

## What passes through during maintenance

The following requests always reach the application regardless of the flag:

| Path prefix | Reason |
|---|---|
| `/actuator/*` | Orchestration health probes (Kubernetes, Fly.io) must keep working so the machine is not killed. |

Everything else is gated — unless you configure explicit exceptions (see below).

---

## CLI options

### `autumn maintenance on`

```
autumn maintenance on [OPTIONS]
```

| Flag | Type | Description |
|---|---|---|
| `--message <MSG>` | string | Message displayed to users in the 503 response. |
| `--allow-ips <CIDR>` | repeatable | One or more IP/CIDR blocks whose traffic passes through unblocked. |
| `--readonly` | flag | Only blocks mutating requests (POST, PUT, PATCH, DELETE). GET, HEAD, and OPTIONS pass through. |
| `--bypass-header <NAME:VALUE>` | string | Requests carrying this exact header name and value bypass maintenance. |

All options are additive — you can combine them in any order.

### `autumn maintenance off`

```
autumn maintenance off
```

Deletes the flag file. If the file is not present (maintenance was not active),
the command prints a warning but exits successfully.

---

## Allow-list options in detail

### `--message`

The message appears in both the HTML and JSON 503 responses. Omit it for a
generic "service unavailable" body, or set it to something actionable:

```bash
autumn maintenance on \
  --message "We are running a planned migration. Back online by 14:30 UTC."
```

### `--allow-ips`

Pass individual IPs or CIDR blocks. Repeat the flag for multiple ranges:

```bash
autumn maintenance on \
  --allow-ips 10.0.0.0/8 \
  --allow-ips 192.168.1.50
```

Traffic from addresses inside any listed range reaches the application normally.
Both IPv4 and IPv6 ranges are accepted. IPv4-mapped IPv6 addresses
(e.g. `::ffff:10.0.0.1`) are matched against the IPv4 block.

Useful when you want your own office or VPN IP to have read access while the
public is locked out.

### `--readonly`

Read-only mode passes GET, HEAD, and OPTIONS through to the application and
returns 503 only for POST, PUT, PATCH, and DELETE:

```bash
autumn maintenance on --readonly \
  --message "We are migrating data. Reads are available; writes are paused."
```

This is ideal when the migration only affects tables that are not read by the UI,
or when you want users to be able to view the site but not submit forms.

### `--bypass-header`

Any request carrying the exact header name and value listed here bypasses
maintenance entirely:

```bash
autumn maintenance on \
  --bypass-header "X-Maintenance-Bypass:my-internal-token"
```

Use this to keep an admin dashboard, a health-check script, or an internal API
consumer working while all other traffic is blocked. Keep the value secret —
it is stored in the flag file on disk.

---

## Response format

The 503 response is content-negotiated based on the `Accept` header.

### HTML (default)

Requests that include `text/html` in `Accept` (a browser, an htmx request)
receive an HTML page:

```html
<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Maintenance</title></head>
<body>
  <h1>Service Unavailable</h1>
  <p>We are running a planned migration. Back online by 14:30 UTC.</p>
  <p>Please try again shortly.</p>
</body>
</html>
```

### JSON (APIs)

Requests that prefer `application/json` (an API client, a mobile app) receive
an [RFC 7807 Problem Details](https://www.rfc-editor.org/rfc/rfc7807) response:

```json
{
  "type": "https://docs.autumn-framework.dev/errors/maintenance",
  "title": "Service Unavailable",
  "status": 503,
  "detail": "We are running a planned migration. Back online by 14:30 UTC."
}
```

Both responses include `Retry-After: 120`, which tells HTTP clients and proxies
to wait at least two minutes before retrying.

---

## Middleware registration

The maintenance middleware ships as `autumn_web::middleware::MaintenanceLayer`.
`autumn_web::app()` registers it automatically — you do not need to add it
manually. The middleware slot sits between the load-shedder and the authentication
layers so unauthenticated traffic is still gated during maintenance.

If you are building a custom app without `autumn_web::app()`, register it
explicitly:

```rust,no_run
use autumn_web::middleware::{MaintenanceLayer, MaintenanceState};

let state = MaintenanceState::default();

let app = Router::new()
    .route("/", get(index))
    .layer(MaintenanceLayer::new(state.clone()));
```

To override the health prefix (default `/actuator`):

```rust,no_run
MaintenanceLayer::new(state)
    .with_health_prefix("/health")
```

---

## `autumn migrate --with-maintenance`

For migrations that need write traffic stopped during the run, pass
`--with-maintenance` to `autumn migrate`. The CLI will:

1. Write the maintenance flag file before running the migration.
2. Run migrations.
3. Remove the flag file on success, reopening traffic.
4. **Leave the flag file in place on failure** and print guidance. You must
   diagnose the failed migration and run `autumn maintenance off` yourself when
   it is safe to reopen traffic.

```bash
autumn migrate --with-maintenance
```

The flag file message is set automatically to
`"Database migration in progress"`. If you want a custom message for the
public-facing 503, run `autumn maintenance on --message "..."` yourself before
calling `autumn migrate` without `--with-maintenance`.

---

## `autumn doctor` integration

`autumn doctor` includes a maintenance-mode check. It reports:

- **PASS** — no flag file present; maintenance is not active.
- **WARN** — flag file found; maintenance is active. The check prints the
  message from the flag file so it is visible in the `doctor` report.

`WARN` is intentional — `autumn doctor` stays green during a planned maintenance
window so CI health scripts can still pass.

---

## `autumn dev` banner

When you start the development server with `autumn dev` while the flag file
exists, the CLI prints a banner before the server output:

```
  ⚠️  MAINTENANCE MODE IS ON
     Message: Database migration in progress
     Run `autumn maintenance off` to disable.
```

This prevents accidentally leaving maintenance on after a local test.

---

## Runbook: destructive migration window

This is the full sequence for a migration that **drops or renames a column** (or
makes any other change that would produce errors if write traffic continued
during the migration).

### Before you start

Confirm you have:

- `autumn-cli` ≥ 0.4.0 installed on the machine that runs the CLI.
- SSH or shell access to the working directory of the running app (or a shared
  volume that all replicas read from).
- The `AUTUMN_DATABASE__PRIMARY_URL` environment variable set to the write
  connection string.

### Step 1 — Enter maintenance

```bash
autumn maintenance on \
  --message "We are running a planned migration. Back online in a few minutes." \
  --allow-ips 10.0.0.0/8
```

Verify the banner appears in the app's log output within 500 ms. Check the
health endpoint (which always passes through):

```bash
curl -i http://localhost:3000/actuator/health
# Expected: 200 OK — the actuator prefix is always allowed
```

Verify that application routes are blocked:

```bash
curl -i http://localhost:3000/
# Expected: 503 Service Unavailable
# Expected header: Retry-After: 120
```

### Step 2 — Run the migration

```bash
AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod" \
  autumn migrate
```

If the migration succeeds, move to Step 3.

If the migration **fails**, leave maintenance on and investigate before removing
the flag. A failed destructive migration may have left the schema in a partial
state. Do not re-open traffic until you have confirmed the schema is consistent.

```bash
# Once the schema is confirmed safe:
autumn maintenance off
```

### Step 3 — Exit maintenance

```bash
autumn maintenance off
```

Traffic resumes within 500 ms. Confirm with:

```bash
curl -i http://localhost:3000/
# Expected: 200 OK (or whatever your root route returns normally)
```

### Step 4 — Verify

Run `autumn doctor` to confirm no residual maintenance state:

```bash
autumn doctor
# Expected: all checks PASS, including "Maintenance mode: PASS"
```

### Automated version (CI/CD pipeline)

`autumn migrate --with-maintenance` condenses Steps 1–3 into a single command.
Use it in your release pipeline when migrations are always safe to run
automatically:

```bash
AUTUMN_DATABASE__PRIMARY_URL="postgres://user:pass@host:5432/myapp_prod" \
  autumn migrate --with-maintenance
```

The flag is automatically removed on success. On failure the flag remains and
the command exits non-zero, failing the pipeline step so the outage window does
not silently close while the schema is broken.

---

## Fly.io deploy integration

For Fly deployments using a `release_command`, pair maintenance with the
migration release command:

```toml
# fly.toml
[deploy]
  release_command = "autumn migrate --with-maintenance"
```

When the release command runs in a temporary Fly machine, it enters maintenance
(blocking traffic on the existing machines via the shared volume), runs
migrations, then exits maintenance. The new machines roll out only after the
release command exits zero. See [deployment.md](deployment.md) for the full
Fly.io setup.

---

## Relation to other safe-deploy features

| Feature | Guide | What it protects |
|---|---|---|
| Migration safety | [deployment.md](deployment.md) | Ensures migrations run before web replicas start (schema-first rollout). |
| Graceful shutdown | [deployment.md](deployment.md) | Ensures in-flight requests complete before the process exits (SIGTERM → drain → exit). |
| Maintenance mode | This guide | Stops new requests from reaching the application while a maintenance operation runs. |

The three features are complementary. A typical zero-downtime destructive
migration uses all three: graceful shutdown ensures no request is abandoned
mid-flight when the old replica exits; migration safety ensures the schema is
updated before new replicas serve traffic; maintenance mode ensures writes are
paused while the migration runs.

---

## Next steps

- **Automate**: wire `autumn migrate --with-maintenance` into your CI/CD
  release pipeline so every deploy automatically manages the maintenance window.
- **Alert**: add a log alert on `"Maintenance mode ENABLED"` in your log
  aggregator so on-call is paged if maintenance is left on unexpectedly.
- **Monitor**: `autumn doctor` can be run as a cron job or a CI step to catch
  a forgotten maintenance flag before it affects users.
