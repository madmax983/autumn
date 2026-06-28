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

## Auto-broadcast with `LiveFragment`

Autumn can broadcast OOB (out-of-band) htmx fragments automatically whenever
a repository mutates a record. Opt in by implementing `LiveFragment` on your
model and adding `broadcasts = true` to `#[repository]`.

Requires the `ws`, `maud`, and `htmx` Cargo features.

### 1. Implement `LiveFragment`

```rust
use autumn_web::prelude::*;
use maud::{Markup, html};

pub struct Post { pub id: i64, pub title: String }

impl LiveFragment for Post {
    fn dom_id_for(id: i64) -> String {
        format!("post-{id}")
    }

    fn dom_id(&self) -> String {
        Self::dom_id_for(self.id)
    }

    fn render_fragment(&self) -> Markup {
        html! {
            li id=(self.dom_id()) { (self.title) }
        }
    }

    // Override for inserts: append to a list container instead of
    // trying to replace an element that doesn't exist yet on the client.
    fn insert_swap() -> OobSwap {
        OobSwap::Target(OobMethod::BeforeEnd, "#posts-list".to_string())
    }
}
```

### 2. Declare `broadcasts = true` on the repository

```rust
#[repository(Post, broadcasts = true)]
pub trait PostRepository {}
```

Use `topic = "custom-name"` to override the default topic (which is the
table name, e.g. `"posts"`). Broadcasts fire synchronously after each
`save`, `update`, or `delete_by_id` call.

### 3. Wire the SSE list container in your template

```html
<ul id="posts-list"
    hx-ext="sse"
    sse-connect="/posts/stream"
    sse-swap="message"
    hx-swap="none">
  <!-- rows rendered server-side on initial load -->
</ul>
```

`hx-swap="none"` is required — it disables htmx's default in-band
`innerHTML` swap so that incoming SSE messages are processed only as OOB
swaps (instead of clearing the list first).

### 4. Add the SSE stream route

```rust
#[get("/posts/stream")]
async fn stream(State(state): State<AppState>) -> impl IntoResponse {
    autumn_web::sse::stream(&state, "posts")
}
```

### OOB swap strategies

| Mutation | Default strategy |
|----------|-----------------|
| `save`   | `LiveFragment::insert_swap()` — defaults to `OobSwap::True` (replace by id); override with `OobSwap::Target` to append to a container |
| `update` | `OobSwap::OuterHTML` (replace element by matching id) |
| `delete` | `OobSwap::Delete` (remove element from DOM) |

When both `broadcasts = true` and `commit_hooks = true` are declared,
broadcasts fire in the durable commit-hook worker (after DB commit). Without
commit hooks, they fire inline after the mutation in the same async task.

---

## `--live` Scaffold

`autumn generate scaffold` accepts a `--live` flag that wires up a
complete SSE broadcast pipeline for you:

```sh
autumn generate scaffold Post title:String body:String --live
```

This emits:
- A `LiveFragment` impl on the model.
- `#[repository(Post, broadcasts = true)]` on the generated repository.
- A `/posts/stream` SSE route.
- An index template whose list container uses `hx-ext="sse"` with
  `hx-swap="none"` so OOB fragments patch rows without clearing the list.
- The idiomorph `<script>` in the layout and `hx-ext="morph"` on `<body>`
  for smooth DOM morphing on full-page navigations.

Combine `--live` with `--live-validation` to also generate per-field
`hx-post` validators that run server-side rules on `change` events and
render inline error spans.

---

## Presence helpers

See the [Presence guide](presence.md) for `presence_stream` (an SSE stream
of join/leave events with OOB count badges) and `presence_badge` (a
re-usable viewer-count fragment). Both integrate with the same channels
infrastructure described here.

---

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
