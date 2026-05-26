//! Runtime configuration example.
//!
//! Demonstrates a tunable rate-limit ceiling via the runtime config store.
//!
//! # Run
//!
//! ```bash
//! cargo run -p runtime-config
//! ```
//!
//! In another terminal, inspect and change config:
//!
//! ```bash
//! # Show all keys and their current values
//! curl http://localhost:3000/config
//!
//! # Read a single key (falls back to compile-time default = 100.0)
//! curl http://localhost:3000/config/rate_limit_rps
//!
//! # Change the ceiling in-process (no restart required)
//! curl -X POST "http://localhost:3000/config/rate_limit_rps?value=50.0"
//!
//! # Verify the change took effect immediately
//! curl http://localhost:3000/config/rate_limit_rps
//!
//! # Revert to the compile-time default
//! curl -X DELETE http://localhost:3000/config/rate_limit_rps
//! ```

use autumn_web::prelude::*;
use autumn_web::runtime_config::{
    ConfigKeySchema, ConfigRegistry, ConfigValidator, ConfigValue, ConfigValueType,
    InMemoryConfigStore, RuntimeConfigService,
};
use std::sync::Arc;

// ── Config setup ──────────────────────────────────────────────────────────────

fn build_registry() -> ConfigRegistry {
    let mut r = ConfigRegistry::new();

    r.define(
        ConfigKeySchema::new(
            "rate_limit_rps",
            ConfigValueType::Float,
            ConfigValue::Float(100.0),
        )
        .description("Global inbound request rate limit in requests-per-second")
        .validator(ConfigValidator::FloatRange {
            min: Some(0.1),
            max: Some(10_000.0),
        }),
    )
    .expect("rate_limit_rps schema is valid");

    r.define(
        ConfigKeySchema::new(
            "support_email",
            ConfigValueType::Text,
            ConfigValue::Text("support@example.com".to_owned()),
        )
        .description("Reply-to address used in outbound support emails"),
    )
    .expect("support_email schema is valid");

    r.define(
        ConfigKeySchema::new(
            "maintenance_banner",
            ConfigValueType::Bool,
            ConfigValue::Bool(false),
        )
        .description("When true, show a maintenance banner on every page"),
    )
    .expect("maintenance_banner schema is valid");

    r.define(
        ConfigKeySchema::new(
            "request_timeout_secs",
            ConfigValueType::DurationSecs,
            ConfigValue::DurationSecs(30),
        )
        .description("Outbound HTTP request timeout in seconds"),
    )
    .expect("request_timeout_secs schema is valid");

    r
}

// ── Route handlers ────────────────────────────────────────────────────────────
//
// State is shared via `Arc<RuntimeConfigService>` stored in a module-level
// `OnceLock`.  In production apps, store it in the Axum extension layer or
// your own application state type.

use std::sync::OnceLock;
static SVC: OnceLock<Arc<RuntimeConfigService>> = OnceLock::new();

fn svc() -> &'static RuntimeConfigService {
    SVC.get().expect("RuntimeConfigService initialised in main")
}

/// GET / — show a friendly summary including the live rate-limit ceiling.
#[get("/")]
async fn index() -> String {
    let rps = svc()
        .get("rate_limit_rps")
        .ok()
        .and_then(|v| v.as_float())
        .unwrap_or(100.0);
    format!(
        "Runtime Config Demo\n\
         Current rate limit: {rps} rps\n\
         \n\
         Endpoints:\n\
           GET  /config                       - list all keys\n\
           GET  /config/:key                  - get a single key\n\
           POST /config/:key?value=<v>        - set a key\n\
           DELETE /config/:key                - revert to default\n"
    )
}

/// GET /config — list all keys with their current values.
#[get("/config")]
async fn list_config() -> String {
    let entries = svc().list();
    let mut lines = vec!["Runtime config:".to_owned()];
    for e in &entries {
        let marker = if e.is_overridden {
            "[overridden]"
        } else {
            "[default]  "
        };
        lines.push(format!(
            "  {marker} {} = {}  (type: {})",
            e.name, e.current, e.value_type,
        ));
    }
    lines.join("\n")
}

/// GET /config/:key — return the current value for a single key.
#[get("/config/{key}")]
async fn get_config(Path(key): Path<String>) -> String {
    match svc().get(&key) {
        Ok(v) => v.to_raw(),
        Err(e) => format!("error: {e}"),
    }
}

/// POST /config/:key?value=<v> — set a key to a new value.
#[post("/config/{key}")]
async fn set_config(
    Path(key): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> String {
    let Some(value) = params.get("value") else {
        return "error: missing ?value= query parameter".to_owned();
    };
    match svc().set(&key, value, Some("api")) {
        Ok(()) => format!("OK: set {key} = {value}"),
        Err(e) => format!("error: {e}"),
    }
}

/// DELETE /config/:key — revert a key to its compile-time default.
#[delete("/config/{key}")]
async fn unset_config(Path(key): Path<String>) -> String {
    match svc().unset(&key, Some("api")) {
        Ok(()) => format!("OK: unset {key} (reverted to compile-time default)"),
        Err(e) => format!("error: {e}"),
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[autumn_web::main]
async fn main() {
    let registry = Arc::new(build_registry());
    let store = Arc::new(InMemoryConfigStore::new());
    SVC.set(Arc::new(RuntimeConfigService::new(registry, store)))
        .expect("SVC initialised once");

    autumn_web::app()
        .routes(routes![
            index,
            list_config,
            get_config,
            set_config,
            unset_config
        ])
        .run()
        .await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_svc() -> RuntimeConfigService {
        let registry = Arc::new(build_registry());
        let store = Arc::new(InMemoryConfigStore::new());
        RuntimeConfigService::new(registry, store)
    }

    #[test]
    fn rate_limit_defaults_to_100() {
        let svc = test_svc();
        let v = svc.get("rate_limit_rps").unwrap();
        assert_eq!(v, ConfigValue::Float(100.0));
    }

    #[test]
    fn set_rate_limit_and_read_back() {
        let svc = test_svc();
        svc.set("rate_limit_rps", "50.5", Some("test")).unwrap();
        let v = svc.get("rate_limit_rps").unwrap();
        assert_eq!(v, ConfigValue::Float(50.5));
    }

    #[test]
    fn unset_rate_limit_reverts_to_default() {
        let svc = test_svc();
        svc.set("rate_limit_rps", "200.0", None).unwrap();
        svc.unset("rate_limit_rps", None).unwrap();
        let v = svc.get("rate_limit_rps").unwrap();
        assert_eq!(v, ConfigValue::Float(100.0));
    }

    #[test]
    fn rate_limit_out_of_range_is_rejected() {
        let svc = test_svc();
        assert!(svc.set("rate_limit_rps", "0.0", None).is_err());
        assert!(svc.set("rate_limit_rps", "99999.0", None).is_err());
        // Value unchanged after rejected writes
        assert_eq!(
            svc.get("rate_limit_rps").unwrap(),
            ConfigValue::Float(100.0)
        );
    }

    #[test]
    fn set_invalid_type_is_rejected() {
        let svc = test_svc();
        assert!(svc.set("rate_limit_rps", "not-a-float", None).is_err());
    }

    #[test]
    fn set_unknown_key_is_rejected() {
        let svc = test_svc();
        assert!(svc.set("nonexistent_key", "42", None).is_err());
    }

    #[test]
    fn list_shows_all_keys_sorted() {
        let svc = test_svc();
        let entries = svc.list();
        assert_eq!(entries.len(), 4);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list() must return sorted entries");
        assert!(names.contains(&"rate_limit_rps"));
        assert!(names.contains(&"support_email"));
    }

    #[test]
    fn history_records_mutations() {
        let svc = test_svc();
        svc.set("support_email", "alice@example.com", Some("alice"))
            .unwrap();
        svc.set("support_email", "bob@example.com", Some("bob"))
            .unwrap();
        let history = svc.history("support_email", 10);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].actor.as_deref(), Some("bob"));
        assert_eq!(history[1].actor.as_deref(), Some("alice"));
    }
}
