//! Runtime configuration registry for reddit-clone.
//!
//! Keys defined here are tunable live — via the `autumn config` CLI or the
//! admin plugin's config page — without restarting the application.

use autumn_web::runtime_config::{
    ConfigKeySchema, ConfigRegistry, ConfigValidator, ConfigValue, ConfigValueType,
};

/// Build the application config registry.
pub fn build_registry() -> ConfigRegistry {
    let mut r = ConfigRegistry::new();

    r.define(
        ConfigKeySchema::new("posts_per_page", ConfigValueType::Int, ConfigValue::Int(25))
            .validator(ConfigValidator::IntRange {
                min: Some(1),
                max: Some(100),
            })
            .description("Number of posts shown on the front page and subreddit pages"),
    )
    .expect("posts_per_page schema is valid");

    r.define(
        ConfigKeySchema::new(
            "registration_open",
            ConfigValueType::Bool,
            ConfigValue::Bool(true),
        )
        .description("When false, new user registrations are rejected"),
    )
    .expect("registration_open schema is valid");

    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::runtime_config::{InMemoryConfigStore, RuntimeConfigService};
    use std::sync::Arc;

    fn svc() -> RuntimeConfigService {
        RuntimeConfigService::new(
            Arc::new(build_registry()),
            Arc::new(InMemoryConfigStore::new()),
        )
    }

    #[test]
    fn posts_per_page_defaults_to_25() {
        assert_eq!(svc().get("posts_per_page").unwrap(), ConfigValue::Int(25));
    }

    #[test]
    fn registration_open_defaults_to_true() {
        assert_eq!(
            svc().get("registration_open").unwrap(),
            ConfigValue::Bool(true)
        );
    }

    #[test]
    fn posts_per_page_can_be_changed_at_runtime() {
        let s = svc();
        s.set("posts_per_page", "50", None).unwrap();
        assert_eq!(s.get("posts_per_page").unwrap(), ConfigValue::Int(50));
        s.unset("posts_per_page", None).unwrap();
        assert_eq!(s.get("posts_per_page").unwrap(), ConfigValue::Int(25));
    }

    #[test]
    fn posts_per_page_rejects_values_outside_query_limit_range() {
        let s = svc();
        for raw in ["-1", "0", "101"] {
            assert!(
                s.set("posts_per_page", raw, None).is_err(),
                "posts_per_page={raw} should be rejected before it reaches Diesel LIMIT"
            );
        }
    }

    #[test]
    fn registration_can_be_closed_at_runtime() {
        let s = svc();
        s.set("registration_open", "false", None).unwrap();
        assert_eq!(
            s.get("registration_open").unwrap(),
            ConfigValue::Bool(false)
        );
    }
}
