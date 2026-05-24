use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const FILTERED_PLACEHOLDER: &str = "[FILTERED]";

pub const DEFAULT_FILTER_KEYS: &[&str] = &[
    "password",
    "password_confirmation",
    "token",
    "secret",
    "authorization",
    "api_key",
    "access_token",
    "refresh_token",
    "cookie",
    "set-cookie",
    "ssn",
    "credit_card",
    "card_number",
    "cvv",
];

#[derive(Debug, Clone)]
pub struct ParameterFilter {
    keys: BTreeSet<String>,
}

impl Default for ParameterFilter {
    fn default() -> Self {
        Self::new(&[], &[])
    }
}

impl ParameterFilter {
    pub fn new(additional: &[String], opt_out_defaults: &[String]) -> Self {
        let mut keys = BTreeSet::new();
        for key in DEFAULT_FILTER_KEYS {
            if !opt_out_defaults.iter().any(|v| v.eq_ignore_ascii_case(key)) {
                keys.insert(key.to_ascii_lowercase());
            }
        }
        for key in additional {
            keys.insert(key.to_ascii_lowercase());
        }
        Self { keys }
    }

    pub fn scrub_json(&self, value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(self.scrub_map(map)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.scrub_json(v)).collect()),
            _ => value.clone(),
        }
    }

    fn scrub_map(&self, map: &Map<String, Value>) -> Map<String, Value> {
        let mut out = Map::new();
        for (key, value) in map {
            if self.matches_key(key) {
                out.insert(key.clone(), Value::String(FILTERED_PLACEHOLDER.to_owned()));
            } else {
                out.insert(key.clone(), self.scrub_json(value));
            }
        }
        out
    }

    pub fn matches_key(&self, key: &str) -> bool {
        let k = key.to_ascii_lowercase();
        self.keys.iter().any(|item| item == &k || k.contains(item))
    }
}

pub fn scrub(value: &Value) -> Value {
    ParameterFilter::default().scrub_json(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn red_default_password_is_scrubbed() {
        let payload = json!({"password":"hunter2","email":"user@example.com"});
        let out = scrub(&payload);
        assert_eq!(out["password"], FILTERED_PLACEHOLDER);
        assert_eq!(out["email"], "user@example.com");
    }

    #[test]
    fn case_insensitive_and_substring_match() {
        let filter = ParameterFilter::default();
        assert!(filter.matches_key("PASSWORD"));
        assert!(filter.matches_key("customer_password"));
        assert!(filter.matches_key("auth_token_v2"));
    }

    #[test]
    fn additive_custom_key_and_opt_out() {
        let filter = ParameterFilter::new(&["pin".to_owned()], &["password".to_owned()]);
        let payload = json!({"password":"open","pin":"1234"});
        let out = filter.scrub_json(&payload);
        assert_eq!(out["password"], "open");
        assert_eq!(out["pin"], FILTERED_PLACEHOLDER);
    }
}
