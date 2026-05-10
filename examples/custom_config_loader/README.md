# Autumn Custom Config Loader Example

Demonstrates replacing Autumn's default TOML + environment-variable config
loader with a custom implementation of the `ConfigLoader` trait. The example
uses a JSON file as the config source; the same pattern works for AWS Secrets
Manager, HashiCorp Vault, Consul, or any other external source.

## What it demonstrates

| Feature | Where | What it does |
|---------|-------|--------------|
| `ConfigLoader` trait | `src/main.rs` | Defines the contract for custom config loading |
| `AppBuilder::with_config_loader` | `src/main.rs` | Swaps in the custom loader before the app boots |
| `AutumnConfig` | `src/main.rs` | The framework config struct that the loader must produce |
| JSON config file | `config.json` | Example non-TOML config source (port 4567, pretty logging) |

## Prerequisites

- Rust 1.88.0+

No database or external services required.

## Quick start

From the **workspace root** (`autumn/`):

```bash
cargo run -p custom-config-loader-example
```

The server starts on `http://127.0.0.1:4567` (port comes from `config.json`,
not from the default `autumn.toml` chain).

### Prove it works

```bash
curl http://127.0.0.1:4567/
# => Hello from autumn — booted with configuration loaded from config.json via a custom ConfigLoader.
```

## Adapting to other config sources

Replace `JsonFileConfigLoader::load` with any async I/O that returns an
`AutumnConfig`. For example:

- **AWS Secrets Manager**: fetch a secret by ARN and deserialize the JSON value.
- **HashiCorp Vault**: call the Vault HTTP API and map the secret data.
- **Consul**: read a KV key and deserialize.
- **HTTP endpoint**: use `reqwest` to fetch a JSON config endpoint at startup.

The framework runs the rest of the boot sequence identically regardless of
which loader you supply.

## Available routes

| Method | Path | Response |
|--------|------|----------|
| GET | `/` | Greeting confirming JSON-sourced config |
| GET | `/health` | `{"status":"UP"}` |
| GET | `/actuator/health` | Extended health JSON |
