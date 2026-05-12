# ws-echo

Minimal real-time Autumn example covering:

- `/echo` - WebSocket echo
- `/chat` - WebSocket fan-out through `AppState::channels()`
- `/` - htmx page with a Maud-rendered live list
- `/items` - form post that broadcasts a new list item
- `/events` - SSE subscription through `autumn_web::sse::stream`
- `/notify` - script-friendly Maud list-item broadcast used by the smoke test

## Prerequisites

- Rust 1.88.0+ (local run)
- Docker and Docker Compose (Redis fan-out smoke test)

## Quick start

Run one local replica:

```powershell
cargo run -p ws-echo
```

Smoke the SSE path from two terminals:

```powershell
curl.exe -N http://127.0.0.1:3000/events
```

```powershell
curl.exe -X POST http://127.0.0.1:3000/notify
```

The `/items` route accepts the page form and publishes a Maud-rendered list item
wrapped in an `hx-swap-oob` envelope. The SSE stream emits that payload as the
event data, so every connected browser appends the item without handwritten
client JavaScript.

## Redis fan-out

Run the CI-friendly compose smoke:

```bash
docker compose -f examples/ws-echo/docker-compose.yml up --build --abort-on-container-exit --exit-code-from smoke smoke
docker compose -f examples/ws-echo/docker-compose.yml down -v
```

Compose starts Redis plus two `ws-echo` replicas on ports `3001` and `3002`,
both using:

```toml
[channels]
backend = "redis"
capacity = 32

[channels.redis]
url = "redis://127.0.0.1:6379/"
key_prefix = "autumn:ws-echo"
```

The `smoke` service opens `/events` on `app1`, posts `/notify` to `app2`,
and fails unless the SSE stream receives the htmx out-of-band fragment. That
is the CI path; no orphaned local terminals, no ritual candle.

Manual smoke while compose is running:

```bash
curl -N http://127.0.0.1:3001/events
curl -X POST http://127.0.0.1:3002/notify
```

The event opened against port `3001` should receive the publish sent to port
`3002`. If it does, Redis pub/sub is carrying channel traffic across replicas.
