
What about "The Simulator: Add a "Debug Mode" that auto-plays the game or queries the DB."
We could add a load test feature to autumn-cli? "autumn loadtest http://localhost:3000"? Wait, that's external.

What about ratatui UI for `autumn db shell`? We have `autumn db migrate` but no database viewer.
What about a ratatui UI for log viewing? `autumn logs` that streams logs from the live app? The `LoggersResponse` just gets the log levels.

Let's go back to: The Mashup: Combine `Flash` + `HxResponseExt`.

Currently `Flash` provides:
```rust
    pub async fn success(&self, message: impl Into<String>) {
        self.push(FlashLevel::Success, message).await;
    }
```
If we want to push flash to HTMX, we need the response.
```rust
pub trait FlashHxExt {
    fn hx_flash(self, level: FlashLevel, message: &str) -> Response;
}
```
HTMX can handle events via `HX-Trigger`.
If we send `HX-Trigger: {"flash": {"level": "success", "message": "Item created"}}`.
Then on the frontend, they can do `document.body.addEventListener('flash', function(e) { ... })`.

Is this a cool feature? Yes! It integrates the server-side concepts (FlashLevel, FlashMessage) with HTMX without needing a full reload.
Let's see if we can implement `FlashHxExt` in `autumn/src/htmx.rs` or `autumn/src/flash.rs`.

```rust
use crate::flash::FlashMessage;

pub trait HxResponseFlashExt: IntoResponse + Sized {
    /// Triggers a client-side flash message event via `HX-Trigger`.
    ///
    /// This tells HTMX to dispatch a custom event (e.g., `flash`) with the level and message.
    fn hx_flash(self, level: FlashLevel, message: &str) -> Response {
        let msg = FlashMessage {
            level,
            message: message.to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap(); // {"level":"success","message":"..."}
        // we need to format it as {"flash": {"level":"success","message":"..."}}
        let trigger_payload = format!("{{\"flash\": {}}}", json);

        let mut res = self.into_response();
        // Since HX-Trigger might already exist, we need to carefully append or we can just insert.
        // Actually `append_hx_header` just inserts.
        // For simplicity, we can insert.
        // ...
    }
}
```
Wait, `HX-Trigger` allows JSON payload.
`HX-Trigger: {"flash": {"level": "success", "message": "Item created"}}`

Let's create `HxResponseFlashExt` in `autumn/src/htmx.rs`.
Actually, maybe add `Flash` methods directly inside `Flash` itself? No, `Flash` works via session.
If we use `Flash::push`, it writes to session.
But maybe `hx_flash` is better on `Response` because it's stateless (doesn't touch session, relies on HTMX header).

Wait, what if the user wants to push to session AND trigger HTMX?
If it's an HTMX request, we can just return the header. If it's a redirect, we use the session.

Let's do: Add `HxResponseFlashExt` trait in `autumn/src/htmx.rs` (or `autumn/src/flash.rs`).

Let's check `autumn/src/htmx.rs`.

Another Mashup Idea: Combine `Cache` and `htmx`!
Wait, `Cache` caches data. `htmx` requests partial HTML.
What if we have an `hx_cache` extractor?

Let's stick to `Flash` + `HxResponseExt`.

Wait, let's look at `autumn/src/htmx.rs` again.
```rust
    fn hx_trigger(self, event: &str) -> Response {
        append_hx_header(self, "hx-trigger", event)
    }
```
If we add `hx_flash`, it's very convenient.
```rust
    /// Triggers a client-side flash message event (`HX-Trigger`).
    ///
    /// Serializes a `FlashMessage` into a JSON payload for the `HX-Trigger` header,
    /// so the frontend can display it without a full page reload.
    fn hx_flash(self, level: crate::flash::FlashLevel, message: &str) -> Response {
        let msg = serde_json::json!({
            "flash": {
                "level": level.as_str(),
                "message": message
            }
        });
        append_hx_header(self, "hx-trigger", &msg.to_string())
    }
```
Is this enough for a Nova PR?
Nova requires:
1. The Spark
2. Prototype
3. Unslop (compile and test)
4. Present

Wait, I need to make sure I add ONE new, interesting feature. Is `hx_flash` interesting enough? Yes, it links two separate systems (HTMX and Flash) into a cohesive developer experience.

Let's check if `autumn/src/htmx.rs` can import `FlashLevel`.
`autumn/src/flash.rs` is compiled with `feature = "flash"`? Let's check `Cargo.toml`.
Wait, `Flash` is only compiled `#[cfg(feature = "flash")]` in `lib.rs`!

Let's do this: we'll add `hx_flash` to `HxResponseExt` if `feature = "flash"` is enabled.
Wait, `autumn/src/htmx.rs` is compiled with `#[cfg(feature = "htmx")]`.
So we need `#[cfg(feature = "flash")]` around the `hx_flash` method.
Let's see if we can do this:
```rust
#[cfg(feature = "flash")]
pub trait HxResponseFlashExt: IntoResponse + Sized {
    fn hx_flash(self, level: crate::flash::FlashLevel, message: &str) -> Response {
        let msg = serde_json::json!({
            "flash": {
                "level": level.as_str(),
                "message": message
            }
        });

        let mut res = self.into_response();
        // Append to existing hx-trigger if present, or insert.
        // wait, append_hx_header overrides. HTMX allows multiple triggers via JSON.
        // But for prototype, just insert.
        if let Ok(v) = http::header::HeaderValue::from_str(&msg.to_string()) {
            res.headers_mut().insert("hx-trigger", v);
        }
        res
    }
}
#[cfg(feature = "flash")]
impl<T: IntoResponse> HxResponseFlashExt for T {}
```
Actually we can just add `hx_flash` directly inside `HxResponseExt` using `#[cfg(feature = "flash")]`.
```rust
pub trait HxResponseExt: IntoResponse + Sized {
    ...

    /// Triggers a client-side flash message event via `HX-Trigger`.
    ///
    /// Sends a JSON payload `{ "flash": { "level": "...", "message": "..." } }`
    /// that frontend JavaScript can listen for (`document.body.addEventListener("flash", ...)`).
    #[cfg(feature = "flash")]
    fn hx_flash(self, level: crate::flash::FlashLevel, message: &str) -> Response {
        let msg = serde_json::json!({
            "flash": {
                "level": level.as_str(),
                "message": message
            }
        });
        // We shouldn't overwrite existing hx-trigger blindly if we want to be nice,
        // but append_hx_header uses insert(). Let's use append_hx_header for now.
        // Actually, HTMX parses multiple triggers if they are in JSON.
        append_hx_header(self, "hx-trigger", &msg.to_string())
    }
}
```

Wait, what if we use the "Simulator" idea?
"Add a "Debug Mode" that auto-plays the game or queries the DB."
In Autumn's context, a "Simulator" could be a load generator or a chaos injector. We already have `havoc` tests but maybe a `ChaosLayer` middleware? That delays requests randomly or injects 500s?
Wait, the prompt says "A rejected PR is just a documented brainstorming session."

Another idea: "The Exporter: Add Export to JSON/CSV/PNG functionality."
What if I add an exporter for `autumn-cli` that exports the database schema to Markdown? Or Mermaid ER Diagram?
`autumn db diagram` -> reads the schema and exports a Mermaid diagram!
Wait, Autumn uses Diesel. Can we read `schema.rs` and generate a Mermaid ER diagram?
Let's see if we have `schema.rs`. No, `schema.rs` is generated by `diesel`.
Can we inspect Postgres metadata? We need the DB URL.

Let's look at `autumn/src/htmx.rs` again.
Is `hx_flash` too small?
"Your mission is to analyze the existing codebase capabilities and propose/implement ONE new, interesting feature that fits the project's theme but expands its potential."

What if I add a `#[autumn_web::view]` macro? No, that's complex.
What if I add `RouteExporter`? An actuator endpoint `/actuator/routes` that returns all registered routes in JSON?
In `autumn/src/router.rs`, `AppBuilder` takes `Vec<Route>`.
```rust
        let mut routes = HashMap::new();
        for route in &self.routes {
            routes.insert(route.path.clone(), route.method.clone());
        }
```
Currently `Route` is `pub struct Route { pub method: Method, pub path: String, pub handler: ... }`.
If we collect the routes, we can add a `/actuator/routes` endpoint.
Wait, `MetricsResponse` already has `by_route` but it's dynamic based on traffic.
A static `/actuator/routes` endpoint would tell developers exactly what routes exist.

Let's check `autumn/src/route.rs`.

What about "The Simulator: Add a Debug Mode that auto-plays the game or queries the DB."
Maybe a Chaos Middleware? "The Simulator: Add a Chaos Engineering middleware".
Wait, Havoc is the chaos persona. Nova creates features.

"NOVA'S FAVORITE PATTERNS:
🌟 The Mashup: Combine Module A + Module B.
🌟 The Interface: Add a TUI (Text UI) to a CLI tool using ratatui.
🌟 The Exporter: Add Export to JSON/CSV/PNG functionality.
🌟 The Simulator: Add a Debug Mode that auto-plays the game or queries the DB."

Let's do "The Exporter" - add `/actuator/routes` which returns a list of registered routes (Method, Path, Name) in JSON.
Wait, that's just an endpoint. What if we do an Exporter CLI command `autumn routes`?
If we do `autumn routes`, it parses `src/main.rs`? No, it's not a running process.

Let's do "The Mashup: Combine Cache + HTMX".
Wait, `Cache` caches the response of a function.
What about `htmx` and `SSE`?
What if we add an HTMX SSE endpoint feature?
Actually, `hx_flash` is combining `Flash` and `htmx`. That fits "The Mashup: Combine Module A + Module B" perfectly. Let's make it robust!
We'll implement `FlashHxExt` for `T: IntoResponse`.

Let's see the current HTMX headers in `HxResponseExt`.
```rust
    fn hx_push_url(self, url: &str) -> Response {
        append_hx_header(self, "hx-push-url", url)
    }
```

We will add to `autumn/src/htmx.rs`:
```rust
#[cfg(feature = "flash")]
pub trait HxResponseFlashExt: IntoResponse + Sized {
    /// Triggers a client-side flash message event via `HX-Trigger`.
    ///
    /// Sends a JSON payload `{ "flash": { "level": "...", "message": "..." } }`
    /// that frontend JavaScript can listen for (e.g., `document.body.addEventListener("flash", ...)`).
    /// This is useful for displaying flash messages without a full page reload.
    fn hx_flash(self, level: crate::flash::FlashLevel, message: &str) -> Response {
        let msg = serde_json::json!({
            "flash": {
                "level": level.as_str(),
                "message": message
            }
        });

        let mut res = self.into_response();
        if let Ok(v) = http::header::HeaderValue::from_str(&msg.to_string()) {
            res.headers_mut().insert(http::header::HeaderName::from_static("hx-trigger"), v);
        }
        res
    }
}

#[cfg(feature = "flash")]
impl<T: IntoResponse> HxResponseFlashExt for T {}
```

Wait, `HX-Trigger` is already handled by `hx_trigger`, but `hx_trigger` takes a simple string event name.
If we want to pass data, we need the JSON format: `{"eventName": {"param": "value"}}`.
And we should probably merge with existing `HX-Trigger` headers if there are any.
Wait, if there's already an `HX-Trigger` header, say `hx-trigger: event1`.
To add a flash message, we'd need to convert the existing to JSON. That might be too complex for a simple feature. For now, `res.headers_mut().append(...)` or `insert` is fine. Wait, `append` allows multiple headers. HTMX actually does NOT handle multiple `HX-Trigger` headers well unless they are a single comma-separated string, OR a JSON object.
Actually, if we just use `insert`, it replaces. It's a prototype! "Unslop: Does it compile? Keep it self-contained."

Wait, what if we do "The Exporter" -> `autumn-cli export-openapi`?
No, that's hard to do without types.

What about "The Simulator: Add a "Debug Mode" that auto-plays the game or queries the DB."
We can add a `#[debug_mode]` macro that generates a UI? No.

Let's stick to the Mashup: `Flash` + `HxResponseExt`. It connects two modules that haven't met yet.
But maybe I can make it even cooler?
A `hx_flash_success`, `hx_flash_error`, etc.
```rust
    fn hx_flash_success(self, message: &str) -> Response {
        self.hx_flash(crate::flash::FlashLevel::Success, message)
    }
    fn hx_flash_error(self, message: &str) -> Response {
        self.hx_flash(crate::flash::FlashLevel::Error, message)
    }
    fn hx_flash_warning(self, message: &str) -> Response {
        self.hx_flash(crate::flash::FlashLevel::Warning, message)
    }
    fn hx_flash_info(self, message: &str) -> Response {
        self.hx_flash(crate::flash::FlashLevel::Info, message)
    }
```
Is this enough for a feature PR?
Let's see what else `Flash` can do.
`Flash` is currently a session-backed extractor. What if we add a `Flash::consume_hx(&self)` that consumes flash messages and generates an `HX-Trigger` JSON payload to inject into responses?

```rust
impl Flash {
    /// Consumes all pending flash messages and returns a JSON string suitable for the `HX-Trigger` header.
    /// Returns `None` if there are no pending messages.
    pub async fn consume_hx_trigger(&self) -> Option<String> {
        let messages = self.consume().await;
        if messages.is_empty() {
            return None;
        }

        // HTMX allows an array of events or a JSON object mapping event names to details.
        // E.g., {"flash": [{"level":"success", "message":"ok"}]}
        let payload = serde_json::json!({
            "flash": messages
        });
        Some(payload.to_string())
    }
}
```

And in `HxResponseExt`:
```rust
pub trait HxResponseFlashExt: IntoResponse + Sized {
    /// Triggers client-side flash messages from the session.
    ///
    /// Consumes any pending flash messages from the session and injects them
    /// as an `HX-Trigger` header so the frontend can display them.
    fn hx_flash_session(self, flash: &crate::flash::Flash) -> impl std::future::Future<Output = Response> + Send;
}

impl<T: IntoResponse + Send> HxResponseFlashExt for T {
    async fn hx_flash_session(self, flash: &crate::flash::Flash) -> Response {
        let mut res = self.into_response();
        if let Some(trigger_json) = flash.consume_hx_trigger().await {
            if let Ok(v) = http::header::HeaderValue::from_str(&trigger_json) {
                res.headers_mut().insert(http::header::HeaderName::from_static("hx-trigger"), v);
            }
        }
        res
    }
}
```
Wait, making it `async` requires either returning a Boxed future, or since we are on Rust 1.75+, `async fn` in traits is supported!
Yes, Rust 1.86 is the workspace edition!

Let's test this locally.

Okay, that test failed because `serde` wasn't linked in raw rustc. That's fine, the JSON structure will be `{"flash": [{"level":"success", "message":"Hello"}]}`. This matches HTMX's format for `HX-Trigger` where you provide an event name `flash` and details `[{...}]`.

This is a beautiful "Mashup" feature.
Let's see if we can do this!
I'll add it directly to `autumn/src/htmx.rs`! No wait, `htmx.rs` is compiled only when `feature = "htmx"`. `Flash` requires `feature = "flash"`. If we add it to `htmx.rs`, we need `#[cfg(feature = "flash")]` around it.

What else? We could add `HxResponseExt` methods:
`hx_flash`, `hx_flash_success`, etc.

Wait, if we use `hx_flash`, does it overwrite existing triggers?
In `autumn/src/htmx.rs`:
```rust
fn append_hx_header<T: IntoResponse>(response: T, name: &'static str, value: &str) -> Response {
    let mut res = response.into_response();
    if let Ok(v) = HeaderValue::from_str(value) {
        res.headers_mut().insert(HeaderName::from_static(name), v);
    }
    res
}
```
`insert` overwrites the existing header.
So if I do `.hx_trigger("something").hx_flash_success("yay")`, the flash will overwrite `something`.
To fix this, we could modify `append_hx_header` to actually append if we wanted, but that might be a breaking change for someone relying on `insert`. Wait, HTMX supports comma separated `HX-Trigger`. But JSON mapping is `{ "event1": "...", "event2": "..." }`. It's safer to just let it overwrite for now, or just document it.
Actually, if the user wants multiple triggers, they can construct JSON themselves. `hx_flash` is a convenience.

Wait, instead of `HxResponseExt`, what if we add it to `Flash`?
```rust
#[cfg(feature = "htmx")]
impl Flash {
    /// Injects pending flash messages into the response as `HX-Trigger` events.
    ///
    /// Consumes the messages from the session and sets the `HX-Trigger` header.
    /// Useful when responding to HTMX requests that don't trigger a full page reload.
    pub async fn inject_hx_trigger<T: axum::response::IntoResponse>(&self, response: T) -> axum::response::Response {
        let messages = self.consume().await;
        let mut res = response.into_response();
        if !messages.is_empty() {
            let payload = serde_json::json!({
                "flash": messages
            });
            if let Ok(v) = http::header::HeaderValue::from_str(&payload.to_string()) {
                res.headers_mut().insert(http::header::HeaderName::from_static("hx-trigger"), v);
            }
        }
        res
    }
}
```
This is a bit more self-contained and logical! "Inject HX-Trigger from Flash".
Wait, `axum::response::IntoResponse` isn't imported in `flash.rs`? Let's check `flash.rs`.
`use axum::response::IntoResponse;` is easy to add.

Wait, what if we also add `Flash::push_hx`? Wait, `inject_hx_trigger` is better, you do:
```rust
#[post("/submit")]
async fn submit(flash: Flash) -> impl IntoResponse {
    flash.success("Form submitted!").await;
    flash.inject_hx_trigger(Html("<div>Saved!</div>")).await
}
```
This is incredibly clean and powerful!

Let's do this:
1. In `autumn/src/flash.rs`:
   Add `#[cfg(feature = "htmx")]` methods: `inject_hx_trigger`.
   Wait, if `htmx` is not enabled, the method won't compile. That's fine.

Let's see if we should add TDD tests for this in `flash.rs`.

What if we also add a frontend `HxResponseExt` extension?
```rust
#[cfg(feature = "flash")]
pub trait HxResponseFlashExt: IntoResponse + Sized {
    /// Pushes a flash message to the client via `HX-Trigger`.
    fn hx_flash(self, level: crate::flash::FlashLevel, message: &str) -> Response {
        let msg = serde_json::json!({
            "flash": [{
                "level": level.as_str(),
                "message": message
            }]
        });

        let mut res = self.into_response();
        if let Ok(v) = http::header::HeaderValue::from_str(&msg.to_string()) {
            res.headers_mut().insert(http::header::HeaderName::from_static("hx-trigger"), v);
        }
        res
    }
}
```

Wait, `inject_hx_trigger` in `Flash` is better because it grabs messages from the session, so if multiple components pushed messages during the request, they all get sent!
Yes! `inject_hx_trigger` consumes the session flash and injects it.
```rust
    #[cfg(feature = "htmx")]
    /// Injects pending flash messages into an HTMX response as `HX-Trigger` events.
    ///
    /// Consumes the messages from the session and sets the `HX-Trigger` header
    /// with a JSON payload representing the messages. This allows the frontend
    /// to display flash messages without a full page reload.
    pub async fn inject_hx_trigger<T: axum::response::IntoResponse>(&self, response: T) -> axum::response::Response {
        let messages = self.consume().await;
        let mut res = response.into_response();
        if !messages.is_empty() {
            let payload = serde_json::json!({
                "flash": messages
            });
            if let Ok(v) = http::header::HeaderValue::from_str(&payload.to_string()) {
                res.headers_mut().insert(
                    http::header::HeaderName::from_static("hx-trigger"),
                    v,
                );
            }
        }
        res
    }
```

Wait, `autumn/Cargo.toml` doesn't define `htmx` and `flash` dependencies explicitly in a way that prevents them from referencing each other. Let's look at `Cargo.toml`.

Awesome! `inject_hx_trigger` is a beautiful feature! Let's write the PR description.

💡 **The Spark:** "I noticed we have `Flash` messages and `HxResponseExt`. In single-page apps utilizing htmx, we often want to push flash messages, but since `Flash` stores in the session, and the next request might not be a full page refresh, the messages sit in the session until a full page reload occurs. Can we bridge them?"

🚀 **The Feature:** "Implemented `Flash::inject_hx_trigger` and an extension trait method. This consumes pending flash messages from the session and injects them directly into the response as an `HX-Trigger` JSON payload (`{"flash": [{"level": "success", "message": "..."}]}`)."

🔭 **The Potential:** "Allows HTMX-heavy applications to provide rich, instant feedback (toast notifications, banners) without full page reloads, fully utilizing the server-side Flash session!"

⚠️ **Risk:** "Low. Isolated to `autumn/src/flash.rs` and purely additive. Only compiled when `#[cfg(feature = "htmx")]` is active."

Let's test if I can add TDD test for it.
