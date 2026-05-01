# Autumn Plugins

Autumn integrations are packaged as **plugins**: small types that implement
[`autumn_web::Plugin`] and wire themselves into an `AppBuilder` with a single
`build(self, app)` call. Users compose plugins with `.plugin(...)` or the
tuple-taking `.plugins((...))`, and each plugin's `build` runs exactly once.

```rust
use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

struct LiveFeedPlugin;

impl Plugin for LiveFeedPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        app.on_startup(|state| async move {
            tracing::info!(profile = state.profile(), "live feed started");
            Ok(())
        })
    }
}

autumn_web::app()
    .routes(routes![...])
    .plugin(LiveFeedPlugin)
    .run()
    .await;
```

## Naming conventions

| Kind | Crate name | Struct name |
|------|------------|-------------|
| First-party (lives in this repo) | `autumn-<name>-plugin` | `<Name>Plugin` |
| Third-party (lives on crates.io) | `autumn-plugin-<name>` | `<Name>Plugin` |

Third-party crates keep the `autumn-plugin-` prefix so the ecosystem
is easy to search on crates.io. First-party crates reverse the order so
they cluster with the crate they extend.

Every plugin crate should expose its `<Name>Plugin` type at the crate
root along with a `::new()` constructor and `#[must_use]` fluent
configuration methods.

## Authoring a plugin

```rust
use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

pub struct HelloPlugin {
    greeting: String,
}

impl HelloPlugin {
    #[must_use]
    pub fn new() -> Self {
        Self { greeting: "hello".to_owned() }
    }

    #[must_use]
    pub fn greeting(mut self, greeting: impl Into<String>) -> Self {
        self.greeting = greeting.into();
        self
    }
}

impl Default for HelloPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for HelloPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        let greeting = self.greeting;
        app.on_startup(move |_state| {
            let greeting = greeting.clone();
            async move {
                tracing::info!(%greeting, "hello plugin started");
                Ok(())
            }
        })
    }
}
```

Inside `build`, you have the full `AppBuilder` surface:
`on_startup`, `on_shutdown`, `nest`, `with_extension` / `extension`,
`migrations` (with the `db` feature), `routes`, and so on. Prefer
chaining the existing builder methods over reinventing infrastructure.

## Duplicate registration

Two plugins that share the same [`Plugin::name`] cannot both apply to
the same builder. The default name is `std::any::type_name::<Self>()`,
so a second instance of the same plugin type is skipped with a
`tracing::warn!`. Override `name` only if a plugin is genuinely
designed to be registered more than once (rare — most plugins should
accept a `Vec`-shaped input instead).

`name` returns [`Cow<'static, str>`], so plugins can compute a unique
label from runtime configuration without leaking memory:

```rust
use std::borrow::Cow;

impl Plugin for ShardedPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Owned(format!("sharded-plugin:{}", self.shard))
    }
    // ...
}
```

[`Cow<'static, str>`]: https://doc.rust-lang.org/std/borrow/enum.Cow.html

## Object safety

`Plugin::build` consumes `self`, so `Plugin` is **not** object-safe.
This is deliberate: keeping `self` by value lets config methods stay
zero-overhead (no `Box<dyn Fn>` or dynamic dispatch on every call) and
makes the plugin's builder type signature match Autumn's own
consuming-self builder style. Users who need dynamic plugin collections
can hide types behind an explicit enum or build their own trait object.

## Cooperative plugins

A plugin may want to behave differently when another plugin is present
(for example, skipping its own migrations when a sibling already
registered them). Check with [`AppBuilder::has_plugin`]:

```rust
impl Plugin for MyTelemetryPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        if app.has_plugin(std::any::type_name::<OtherTelemetryPlugin>()) {
            tracing::info!("other telemetry plugin already registered; noop");
            return app;
        }
        app.with_extension(self.exporter)
    }
}
```

[`autumn_web::Plugin`]: https://docs.rs/autumn-web/latest/autumn_web/plugin/trait.Plugin.html
[`Plugin::name`]: https://docs.rs/autumn-web/latest/autumn_web/plugin/trait.Plugin.html#method.name
[`AppBuilder::has_plugin`]: https://docs.rs/autumn-web/latest/autumn_web/app/struct.AppBuilder.html#method.has_plugin
