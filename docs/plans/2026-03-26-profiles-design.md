# Profile-Based Configuration Design

**Date:** 2026-03-26
**Status:** Validated (post six-hats review)
**Target:** v0.2.0

## Overview

Environment-aware configuration layering with smart defaults for `dev` and `prod` profiles. Built on Autumn's existing 3-layer config system, adding profile-specific TOML files as a new layer.

## Config Layering Order

```
1. Hardcoded defaults
2. autumn.toml (base)
3. autumn-{profile}.toml (profile override)
4. AUTUMN_* env vars (final override)
```

Each layer is optional. Values from later layers override earlier ones. **Merging is key-level within sections** (deep merge), not section-level replacement — `autumn-dev.toml` only needs to specify the keys it wants to change. If `autumn.toml` has `[database]` with 5 fields and `autumn-dev.toml` has `[database]` with 1 field, the other 4 are preserved. This must be explicitly tested.

## Profile Selection

Three mechanisms, in precedence order:

1. **Env var** — `AUTUMN_PROFILE=dev` (highest priority)
2. **CLI flag** — `cargo run -- --profile dev`
3. **Auto-detect** — `cfg!(debug_assertions)` → `dev`, release build → `prod`

```
cargo run              → dev profile (debug build)
cargo run --release    → prod profile (release build)
AUTUMN_PROFILE=staging cargo run --release  → staging profile (env override)
```

Custom profiles are supported — `AUTUMN_PROFILE=staging` loads `autumn-staging.toml`.

## Smart Defaults

When a profile is active, these defaults apply **before** any TOML or env var overrides:

| Setting | `dev` | `prod` | No profile |
|---|---|---|---|
| `log.format` | `pretty` | `json` | `auto` (existing behavior) |
| `log.level` | `debug` | `info` | `info` (existing behavior) |
| `server.host` | `127.0.0.1` | `0.0.0.0` | `127.0.0.1` (existing behavior) |
| `health.detailed` | `true` | `false` | `false` (existing behavior) |
| `shutdown.drain_timeout` | `1s` | `30s` | `5s` (existing behavior) |

Smart defaults are inserted between layer 1 (hardcoded defaults) and layer 2 (autumn.toml), so any explicit TOML or env var setting overrides them.

Updated layering with profiles:

```
1. Hardcoded defaults
2. Profile smart defaults (if profile is active)
3. autumn.toml (base)
4. autumn-{profile}.toml (profile override)
5. AUTUMN_* env vars (final override)
```

## User-Facing API

### Basic usage — zero config needed

```rust
#[autumn_web::main]
async fn main() {
    // Profile auto-detected from build mode
    // cargo run → dev (pretty logs, debug level, localhost only)
    // cargo run --release → prod (JSON logs, info level, 0.0.0.0)
    autumn_web::app()
        .routes(routes![index])
        .run()
        .await;
}
```

### Profile-specific config files

`autumn.toml` (base — shared across all profiles):
```toml
[server]
port = 3000

[database]
url = "postgres://localhost/myapp"
pool_size = 10
```

`autumn-dev.toml` (overrides for dev):
```toml
[database]
url = "postgres://localhost/myapp_dev"
```

`autumn-prod.toml` (overrides for prod):
```toml
[server]
port = 8080

[database]
url = "postgres://prod-host/myapp"
pool_size = 50
```

### Accessing the active profile

```rust
// Logged automatically at startup:
// INFO autumn starting (profile: dev, port: 3000)

// Accessible via AppState:
#[get("/debug/info")]
async fn debug_info(state: AppState) -> String {
    format!("profile: {}", state.profile())  // "dev", "prod", "staging", etc.
}
```

### Health endpoint includes profile

```json
{
    "status": "ok",
    "version": "0.1.0",
    "profile": "dev",
    "uptime": "2h 15m"
}
```

When `health.detailed = false` (prod default), profile is hidden:
```json
{
    "status": "ok"
}
```

## Implementation Details

### Profile resolution at startup

**Critical:** The `cfg!(debug_assertions)` check must evaluate in the **user's crate**, not in `autumn_web`'s compiled library code. If it runs inside `autumn_web`, it reflects the library's build mode which may differ from the app's.

Solution: The `#[autumn_web::main]` macro expands in the user's crate, so it evaluates `cfg!(debug_assertions)` there and passes the result to `AppBuilder`:

```rust
// Inside #[autumn_web::main] macro expansion (runs in user's crate):
let is_debug = cfg!(debug_assertions);
autumn_web::app_with_build_mode(is_debug)
    // ...
```

```rust
// In autumn_web library:
pub fn app_with_build_mode(is_debug: bool) -> AppBuilder {
    let profile = resolve_profile(is_debug);
    AppBuilder::new(profile)
}

fn resolve_profile(is_debug: bool) -> String {
    // 1. Check env var (highest priority)
    if let Ok(profile) = std::env::var("AUTUMN_PROFILE") {
        return profile;
    }

    // 2. Check CLI args
    if let Some(profile) = parse_cli_profile() {
        return profile;
    }

    // 3. Auto-detect from build mode (passed from user's crate)
    if is_debug { "dev".to_string() } else { "prod".to_string() }
}
```

### Config loading with profile

```rust
fn load_config(profile: Option<&str>) -> AutumnConfig {
    let mut config = AutumnConfig::defaults();

    // Apply profile smart defaults
    if let Some(profile) = profile {
        config.apply_profile_defaults(profile);
    }

    // Load base autumn.toml
    if let Some(base) = load_toml("autumn.toml") {
        config.merge(base);
    }

    // Load profile-specific TOML
    if let Some(profile) = profile {
        if let Some(profile_toml) = load_toml(&format!("autumn-{profile}.toml")) {
            config.merge(profile_toml);
        }
    }

    // Apply env var overrides
    config.apply_env_overrides();

    config
}
```

### AppState profile access

```rust
impl AppState {
    pub fn profile(&self) -> &str {
        &self.profile
    }
}
```

### Startup log

```
INFO autumn v0.1.0 starting
INFO profile: dev (auto-detected from debug build)
INFO config: autumn.toml + autumn-dev.toml loaded
INFO server: http://127.0.0.1:3000
INFO log level: debug, format: pretty
INFO health: /health (detailed: true)
INFO 2 routes registered, 0 scheduled tasks
```

## Spring Boot Comparison

| Spring Boot | Autumn |
|---|---|
| `application.yml` | `autumn.toml` |
| `application-dev.yml` | `autumn-dev.toml` |
| `SPRING_PROFILES_ACTIVE=dev` | `AUTUMN_PROFILE=dev` |
| No auto-detection | Auto-detect from debug/release build |
| `@Profile("dev")` bean filtering | Not needed — config-driven, not bean-driven |
| Smart defaults per profile | Smart defaults per profile (dev/prod) |
| Profile in Actuator `/info` | Profile in `/health` endpoint |

## Implementation Order

1. **Profile resolution** — env var → CLI flag → auto-detect logic
2. **Smart defaults** — profile-specific default values for dev/prod
3. **Config layering** — insert profile TOML loading between base and env vars
4. **AppState integration** — store active profile, expose via `.profile()`
5. **Health endpoint update** — include profile when `health.detailed = true`
6. **Startup logging** — log profile, config sources, effective settings
7. **Tests** — profile resolution precedence, smart defaults, TOML merging, env override
8. **Documentation** — update config docs with profile usage

## Edge Cases

### No profile TOML file
`AUTUMN_PROFILE=staging` with no `autumn-staging.toml` is fine — smart defaults don't apply to custom profiles, base TOML + env vars still work. A DEBUG log notes the missing file.

### Unknown profile with typo detection
Any string is valid as a profile name. Only `dev` and `prod` have smart defaults. Custom profiles (`staging`, `test`, `ci`) are pure layering — they only get values from their TOML file.

**Typo detection:** If a profile is not `dev` or `prod` AND no matching `autumn-{profile}.toml` file exists, log a WARN:
```
WARN profile "prodd" has no config file (autumn-prodd.toml) and no smart defaults. Did you mean "prod"?
```
Uses Levenshtein distance against known profiles (`dev`, `prod`, plus any `autumn-*.toml` files found) to suggest corrections.

### Profile in tests
`#[autumn_test]` (future feature) could auto-set profile to `test`, loading `autumn-test.toml` with test-specific defaults (e.g., separate database, minimal logging).

### Multiple profiles
Not supported in v0.2. One active profile at a time. Spring Boot supports multiple active profiles but the complexity rarely justifies it.

## Risks & Mitigations (from Six Hats Review)

| Risk | Mitigation |
|---|---|
| `cfg!(debug_assertions)` evaluates in wrong crate (library vs app) | Check runs in `#[autumn_web::main]` macro expansion (user's crate), result passed to `AppBuilder` |
| Profile typo silently falls back to no-profile behavior | WARN log with Levenshtein suggestion when profile has no TOML and isn't dev/prod |
| Deep merge vs shallow merge confusion | Explicit key-level merge within TOML sections; comprehensive test coverage for merge behavior |
| Smart defaults are an invisible layer, hard to debug | `configprops` actuator endpoint shows source of every value including `"profile_default:dev"` |
| Tests behave differently in debug vs release builds | Document that `cargo test` auto-detects `dev`; recommend `AUTUMN_PROFILE=test` for CI consistency |
