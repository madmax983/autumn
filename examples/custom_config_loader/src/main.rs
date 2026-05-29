//! Demonstrates replacing autumn-web's default TOML + env config loader with a
//! custom JSON-file loader via the [`ConfigLoader`] trait and the
//! [`AppBuilder::with_config_loader`](autumn_web::app::AppBuilder::with_config_loader) hook (S-053 success metric).
//!
//! Run with:
//!
//! ```bash
//! cargo run -p custom-config-loader-example
//! ```
//!
//! The example reads `config.json` (sitting next to this `main.rs`) and boots
//! a tiny app whose port and profile come from JSON instead of the framework's
//! default TOML/env layering. Visit `http://127.0.0.1:<port>/` to see the
//! profile reported by the app.
//!
//! The same shape works for any other source — AWS Secrets Manager, Vault,
//! Consul, an HTTP fetch, etc. — by swapping out the body of `JsonFileConfigLoader::load`.

use std::path::PathBuf;

use autumn_web::config::{AutumnConfig, ConfigError, ConfigLoader};
use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str {
    "Hello from autumn — booted with configuration loaded from config.json via a custom ConfigLoader."
}

/// Custom [`ConfigLoader`] backed by a single JSON file on disk.
///
/// Real-world replacements (AWS Secrets Manager, Vault, etc.) follow the same
/// shape: capture whatever inputs you need in `Self`, do the I/O inside `load`,
/// then deserialize into [`AutumnConfig`]. The only contract is "return a
/// fully-resolved `AutumnConfig` or a `ConfigError`" — the framework handles
/// the rest of the boot sequence the same way it would for the default loader.
struct JsonFileConfigLoader {
    path: PathBuf,
}

impl JsonFileConfigLoader {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigLoader for JsonFileConfigLoader {
    /// Load the configuration from the JSON file using asynchronous I/O.
    ///
    /// This prevents blocking the Tokio runtime, which would otherwise occur if
    /// synchronous `std::fs::read` was used. Non-blocking I/O is critical for
    /// scalability and maintaining high responsiveness across all tasks scheduled
    /// on the runtime.
    async fn load(&self) -> Result<AutumnConfig, ConfigError> {
        let bytes = tokio::fs::read(&self.path).await.map_err(ConfigError::Io)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| ConfigError::Validation(format!("invalid JSON config: {e}")))
    }
}

#[autumn_web::main]
async fn main() {
    // Resolve the fixture relative to this crate's manifest so `cargo run`
    // works from any directory in the workspace.
    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.json");

    autumn_web::app()
        .with_config_loader(JsonFileConfigLoader::new(config_path))
        .routes(routes![index])
        .run()
        .await;
}
