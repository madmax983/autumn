### Fixed

- Request-timeout docs no longer promise `408 Request Timeout`: an expired
  server-side deadline returns `503 Service Unavailable` (`408` means the
  client was too slow sending the request).
- The Postgres job backend comment no longer claims `LISTEN`/`NOTIFY`
  wake-ups: workers claim with `FOR UPDATE SKIP LOCKED` on a 200 ms poll
  interval, plus advisory locks for serialized claiming.
- Release templates now use the explicit probe contracts: the ECS ALB target
  group health check and the App Runner cutover probe `/ready` (so the
  drain-time 503 flip reaches the load balancer), and the Dockerfile
  `HEALTHCHECK` defaults to `/live` (pure liveness; a draining or
  pool-exhausted container is no longer restarted). Fly already used this
  split and is pinned by the new `release_templates_use_explicit_probe_contracts`
  hygiene test; the transitional `/health` alias keeps working unchanged.
- `verification/README.md` documents why the Verus specs are manual-only.
