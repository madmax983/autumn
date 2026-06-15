# Realtime Channels, SSE, and htmx Broadcasts

Autumn's `ws` feature provides a named channel registry for WebSockets, SSE,
and server-rendered htmx fragments. Local development uses in-process
`tokio::broadcast` channels. Multi-replica deployments can switch the same
API to Redis pub/sub with `autumn.toml`.

## Enable

```toml
[dependencies]
autumn-web = { version = "0.5", features = ["ws"] }
```

## Publish

Use `AppState::broadcast()` when the payload is intended for browser clients.
`publish` sends raw UTF-8 text. `publish_html` wraps a Maud fragment in an
`hx-swap-oob` envelope for htmx.

```rust,no_run
use autumn_web::prelude::*;

#[post("/tasks/{id}/complete")]
async fn complete(state: AppState, Path(id): Path<i64>) -> AutumnResult<&'static str> {
    state.broadcast().publish_html(
        "tasks",
        &html! {
            li id={ "task-" (id) } class="done" { "complete" }
        },
    )?;

    Ok("ok")
}
```

For protocol payloads that are already encoded, use `publish`:

```rust,no_run
# use autumn_web::prelude::*;
# fn publish(state: AppState) -> AutumnResult<()> {
state.broadcast().publish("tasks", br#"{"type":"task.completed"}"#.as_slice())?;
# Ok(())
# }
```

## Subscribe with SSE

The one-line SSE primitive subscribes to a topic and emits each message as
SSE `data`.

```rust,no_run
use autumn_web::prelude::*;

#[get("/events")]
async fn events(State(state): State<AppState>) -> impl IntoResponse {
    autumn_web::sse::stream(&state, "tasks")
}
```

Use `stream_authorized` when subscription needs access checks. The hook runs
before Autumn allocates the channel subscriber.

```rust,no_run
use autumn_web::prelude::*;

#[get("/events/private")]
async fn private_events(
    State(state): State<AppState>,
    session: Session,
) -> AutumnResult<impl IntoResponse> {
    autumn_web::sse::stream_authorized(&state, "private-tasks", |_| async move {
        if session.contains_key("user_id").await {
            Ok(())
        } else {
            Err(AutumnError::unauthorized_msg("login required"))
        }
    })
    .await
}
```

## Direct Channels

`AppState::channels()` remains the low-level primitive for WebSocket loops and
custom transports.

```rust,no_run
# use autumn_web::prelude::*;
# async fn example(state: AppState) -> AutumnResult<()> {
let tx = state.channels().sender("lobby");
let mut rx = state.channels().subscribe_authorized("lobby", |_| async {
    Ok::<(), AutumnError>(())
}).await?;

tx.send("hello")?;
let _ = rx.recv().await;
# Ok(())
# }
```

## Redis Backend

Local is the default:

```toml
[channels]
backend = "in_process"
capacity = 32
```

Use Redis for multi-replica fan-out:

```toml
[channels]
backend = "redis"
capacity = 128

[channels.redis]
url = "redis://127.0.0.1:6379/"
key_prefix = "autumn:channels"
```

Equivalent environment overrides:

```powershell
$env:AUTUMN_CHANNELS__BACKEND = "redis"
$env:AUTUMN_CHANNELS__CAPACITY = "128"
$env:AUTUMN_CHANNELS__REDIS__URL = "redis://127.0.0.1:6379/"
$env:AUTUMN_CHANNELS__REDIS__KEY_PREFIX = "autumn:channels"
```

The Redis backend publishes locally first, then relays the same envelope over
Redis. Each process ignores messages carrying its own origin id, which avoids
double-delivery on the publishing replica.

## Custom Backends

Implement `autumn_web::channels::ChannelsBackend` and install it with
`AppBuilder::with_channels_backend`. This bypasses config-driven backend
selection, matching the session-store escape hatch.

```rust,no_run
# use autumn_web::prelude::*;
# fn configure(app: autumn_web::app::AppBuilder) -> autumn_web::app::AppBuilder {
app.with_channels_backend(LocalChannelsBackend::new(64))
# }
```

## Actuator Metrics

With the `ws` feature, `/actuator/channels` returns per-topic metrics:

```json
{
  "channels": {
    "tasks": {
      "subscriber_count": 2,
      "lifetime_publish_count": 17,
      "dropped_count": 0,
      "lagged_count": 1
    }
  }
}
```

`dropped_count` increments when a publish has no active local receivers.
`lagged_count` increments when slow subscribers skip messages from the
bounded ring buffer.

## Two-Replica Smoke

The dedicated two-replica smoke test that lived in `examples/ws-echo` has been
consolidated into `examples/reddit-clone`, which demonstrates the same
Redis pub/sub fan-out pattern through its live-feed WebSocket route
(`src/routes/live.rs`). The reddit-clone Docker Compose file only starts the
Postgres and Redis infrastructure; to run a multi-replica smoke you would
start two app instances manually (each pointing at the same Redis and
Postgres) and verify that a post created through one replica appears on a
WebSocket connection opened against the other.
