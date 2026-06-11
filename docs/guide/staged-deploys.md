# Staged and Zero-Downtime Deploys

This guide covers the deploy strategies that Autumn supports out of the box and
explains how the framework's probe contracts, drain lifecycle, and maintenance
mode fit together to give you a safe rollout in each case.

**Strategies covered:**

| Strategy | When to use | Framework support |
|---|---|---|
| Rolling | Standard incremental rollout | Full — probes + drain handle it automatically |
| Blue/green | Instant cutover with easy rollback | Full — probe drain + LB switch |
| Canary | Gradual traffic shift with automated promotion | Needs more infra — see [the tracking issue](#canary-deploys) |

---

## Rolling deploys

A rolling deploy replaces replicas one at a time. The orchestrator (Fly.io,
Kubernetes, Docker Swarm) starts a new replica, waits for it to pass its
readiness check, then terminates an old one. Traffic flows through the healthy
mix of old and new at all times — no downtime window, no manual intervention.

Autumn's probe and drain contracts are designed around this pattern.

### How it works

```
Old replica A  [live] ──────────────────── SIGTERM ─→ drain ─→ exit
Old replica B  [live] ──────────────────────────────────── SIGTERM ─→ drain ─→ exit
New replica C          [starting] ─→ [ready] ─→ [live, serving traffic]
New replica D                             [starting] ─→ [ready] ─→ [live, serving traffic]
```

The key invariant: **`/ready` flips to 503 before the listener closes.** This
gives the load balancer time to deregister the old replica before it stops
accepting connections. No request hits a closing socket.

Concretely, on SIGTERM:

1. `/ready` → 503 immediately (load balancer stops routing new requests here).
2. `prestop_grace_secs` (default 5 s) — time for the load balancer to drain its
   connection pool to this replica.
3. The TCP listener closes.
4. In-flight requests complete (up to `shutdown_timeout_secs`, default 30 s).
5. App hooks, telemetry flush, DB pool close, process exits.

A new replica is only promoted to live after `/ready` returns 200 — which
Autumn gates until the DB connection pool is established and any startup probes
have passed.

### Config knobs

```toml
# autumn.toml
[server]
prestop_grace_secs   = 5    # wait for LB to drain before closing listener
shutdown_timeout_secs = 30  # max time for in-flight requests to complete
```

For Fly.io, `kill_timeout` in `fly.toml` must be at least
`prestop_grace_secs + shutdown_timeout_secs + buffer`:

```toml
# fly.toml
[deploy]
  kill_timeout = 45   # 5 + 30 + 10 s buffer
```

### Migration safety

Schema migrations must run **before** new replicas start. An incompatible
schema during the rollout causes errors on old replicas. The safe sequence:

```bash
# 1. Run migrations (schema changes land before any replica restarts)
autumn migrate

# 2. Deploy new replicas (rolling, one at a time)
fly deploy
```

For destructive schema changes (column drops, renames), use the
expand/contract pattern: add the new column in one deploy, migrate data,
drop the old column in a later deploy when no live code references it anymore.
`autumn migrate check` classifies SQL statements by rolling-deploy risk before
you run them — see [deployment.md](deployment.md).

### Runnable repro

```bash
# Watch /ready flip during a local graceful shutdown
curl -s http://localhost:3000/ready   # → 200
kill -TERM $(pgrep myapp)
curl -s http://localhost:3000/ready   # → 503 (within prestop_grace_secs window)
# In-flight requests complete; process exits after shutdown_timeout_secs
```

---

## Blue/green deploys

A blue/green deploy keeps two complete environments alive simultaneously — the
current live environment (blue) and the new environment (green). Traffic is
switched atomically at the load balancer. Rollback is a second switch back to
blue, with no re-deploy.

This is the right choice when:

- You want instant rollback without re-deploying the old image.
- Your migration is not backward-compatible and you need to keep the old schema
  live until you are confident the new version is healthy.
- You are switching a major dependency (Postgres version, Redis version) and
  want the old stack available for comparison.

### Architecture

```
                         ┌─────────────────────────────┐
Internet ──→ LB ──────→  │  Blue  (current, 100% traffic)│
                         └─────────────────────────────┘
                         ┌─────────────────────────────┐
                         │  Green (new, 0% traffic)     │  ← warming up
                         └─────────────────────────────┘
```

After the switch:

```
                         ┌─────────────────────────────┐
                         │  Blue  (old, 0% traffic)     │  ← idle, available for rollback
                         └─────────────────────────────┘
                         ┌─────────────────────────────┐
Internet ──→ LB ──────→  │  Green (new, 100% traffic)   │
                         └─────────────────────────────┘
```

### Procedure

**Step 1 — Stand up the green environment**

Deploy the new image to a separate set of replicas. Do not send traffic yet.

```bash
# Fly example: deploy to a separate app
fly deploy --app myapp-green

# Kubernetes example: apply to a second Deployment
kubectl apply -f deploy/green.yaml
```

**Step 2 — Warm up and verify green**

Green replicas must pass all three probes before you switch traffic:

```bash
# Startup probe — passes once once the binary is listening
curl -f https://myapp-green.internal/startup

# Liveness probe — passes when the process is healthy
curl -f https://myapp-green.internal/live

# Readiness probe — passes when DB pool is up and ready to serve
curl -f https://myapp-green.internal/ready
```

Run your smoke suite against the green environment directly (before any traffic
switch). Autumn's `/actuator/health` returns the DB pool status and replica lag
so you can confirm the green environment has a working database connection:

```bash
curl -s https://myapp-green.internal/actuator/health | jq .
```

**Step 3 — Run migrations (if any)**

Migrations must target the same database as both environments. Run them before
switching traffic so green replicas start with the new schema already in place:

```bash
autumn migrate
```

If the migration is destructive and the old blue code cannot run against the new
schema, put blue into maintenance mode while migrating:

```bash
autumn maintenance on --message "Upgrading — back in a few minutes."
autumn migrate
```

See [maintenance-mode.md](maintenance-mode.md) for the full runbook.

**Step 4 — Switch traffic**

Redirect 100% of traffic from blue to green at the load balancer:

```bash
# Fly example: update the DNS / Fly anycast IP to point at green
fly ips assign --app myapp-green $(fly ips list --app myapp --json | jq -r '.[0].Address')

# Kubernetes example: flip the Service selector
kubectl patch service myapp -p '{"spec":{"selector":{"version":"green"}}}'
```

**Step 5 — Drain blue**

Blue replicas are still running but no longer receiving traffic. Leave them
running for a rollback window (typically 10–30 minutes), then shut them down:

```bash
# Fly example
fly scale count 0 --app myapp-blue

# Kubernetes example
kubectl scale deployment myapp-blue --replicas=0
```

Because blue's `/ready` endpoint has already been deregistered from the LB,
you can stop the blue processes immediately with no drain needed — no traffic is
flowing to them.

**Rollback**

If green is unhealthy, switch the LB back to blue before stopping it. Blue
never lost its database connection and its code was never changed — it is live
immediately:

```bash
kubectl patch service myapp -p '{"spec":{"selector":{"version":"blue"}}}'
```

Then tear down green, diagnose the issue, and re-deploy when ready.

### Key points

- Autumn's probe contracts give you a deterministic signal for when green is
  ready (`/ready` → 200) and when blue has finished draining (process exits
  cleanly after SIGTERM).
- The framework does not manage the LB switch — that is an operator or platform
  concern. Autumn gives you the health signals; you decide when to pull the
  lever.
- Use `autumn doctor` on the green environment before switching traffic to catch
  misconfigured secrets, missing database URLs, or active maintenance flags:

  ```bash
  autumn doctor
  ```

---

## Canary deploys

A canary deploy routes a small percentage of traffic to a new version while the
rest continues to hit the old version. Automated metrics (error rate, latency
p99) gate promotion — if the canary looks healthy, traffic weight shifts
gradually to 100%; if it degrades, the canary is rolled back automatically.

**Autumn does not have built-in canary routing support yet.** The framework
provides the health check and drain primitives that canary controllers depend
on, but it does not expose a traffic-weight configuration or a canary
promotion/rollback primitive.

Platform-level canary (Fly.io machine weights, Kubernetes `TrafficPolicy`,
Nginx upstream `weight`) works today if your platform supports it, but Autumn
has no framework-level hooks to influence the split or react to canary metrics.

A tracking issue covers what needs to be built:
[#916 — Canary deploy support: traffic routing primitive and promotion hooks](https://github.com/madmax983/autumn/issues/916).

In the meantime, a conservative rolling deploy with `autumn migrate check` as a
pre-deploy gate, and blue/green with a manual traffic switch, covers the
majority of safe-deploy use cases.

---

## Choosing a strategy

| Situation | Recommended strategy |
|---|---|
| Routine feature release, backward-compatible schema | Rolling |
| Destructive schema change (column drop, rename) | Rolling + expand/contract migration pattern |
| High-risk release requiring fast rollback | Blue/green |
| Incident response — stop traffic immediately | [Maintenance mode](maintenance-mode.md) |
| Gradual rollout with automated promotion | Wait for canary support, or use platform-level weights manually |

---

## Next steps

- **Verify before you ship**: `autumn migrate check` classifies SQL by
  rolling-deploy risk. Wire it into CI before `autumn migrate` runs.
- **Harden startup**: `autumn doctor --strict` in CI catches misconfigured
  secrets and missing environment variables before the image is built.
- **Monitor drains**: the `autumn_shutdown_aborted_requests_total` metric
  increments when a request is abandoned during shutdown. Alert on it to catch
  an undersized `shutdown_timeout_secs`.
- **Full cloud-native setup**: Kubernetes readiness probes, OTLP tracing, and
  structured logging are covered in the [Cloud-Native Guide](cloud-native.md).
