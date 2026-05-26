# runtime-config

Live-tunable typed configuration store example for Autumn.

Demonstrates how to declare typed config keys with defaults and validators,
then read, update, and revert values at runtime — no restart required.

## Prerequisites

- Rust 1.88.0+

## Quick Start

```bash
cargo run -p runtime-config
```

In another terminal:

```bash
# List all keys and their current values
curl http://localhost:3000/config

# Read a single key (returns the compile-time default = 100.0)
curl http://localhost:3000/config/rate_limit_rps

# Change the ceiling in-process
curl -X POST "http://localhost:3000/config/rate_limit_rps?value=50.0"

# Verify the change took effect immediately
curl http://localhost:3000/config/rate_limit_rps

# Revert to the default
curl -X DELETE http://localhost:3000/config/rate_limit_rps
```

## What It Shows

- **`ConfigRegistry`** — declare keys with `ConfigKeySchema::new`, add descriptions
  and validators via the builder API.
- **`ConfigValidator`** — built-in `FloatRange`, `IntRange`, `AllowedValues`,
  and `Regex` validators; the first rejection wins.
- **`InMemoryConfigStore`** — thread-safe in-process store for dev and tests.
- **`RuntimeConfigService`** — typed `get` / `set` / `unset` / `list` API; parses
  raw strings, validates, and falls back to schema defaults when unset.
