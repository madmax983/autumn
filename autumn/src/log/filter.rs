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
    normalized_keys: BTreeSet<String>,
}

impl Default for ParameterFilter {
    fn default() -> Self {
        Self::new(&[], &[])
    }
}

impl ParameterFilter {
    pub fn new(additional: &[String], opt_out_defaults: &[String]) -> Self {
        let opt_out_defaults: BTreeSet<String> = opt_out_defaults
            .iter()
            .filter_map(|key| normalize_key(key))
            .collect();

        let mut keys = BTreeSet::new();
        for key in DEFAULT_FILTER_KEYS {
            let Some(normalized_default) = normalize_key(key) else {
                continue;
            };
            if !opt_out_defaults.contains(&normalized_default) {
                keys.insert(key.to_ascii_lowercase());
            }
        }
        for key in additional {
            if let Some(key) = normalize_key(key) {
                keys.insert(key);
            }
        }

        let normalized_keys = keys
            .iter()
            .filter_map(|k| normalize_key(k))
            .collect::<BTreeSet<_>>();

        Self {
            keys,
            normalized_keys,
        }
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
        let Some(normalized) = normalize_key(key) else {
            return false;
        };
        self.keys.contains(&normalized) || self.normalized_keys.contains(&normalized)
    }
}

fn normalize_key(key: &str) -> Option<String> {
    let normalized: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub fn normalized_opt_out_defaults(opt_out_defaults: &[String]) -> Vec<String> {
    let defaults: BTreeSet<String> = DEFAULT_FILTER_KEYS
        .iter()
        .filter_map(|key| normalize_key(key))
        .collect();

    let mut result: Vec<String> = opt_out_defaults
        .iter()
        .filter_map(|key| normalize_key(key))
        .filter(|key| defaults.contains(key))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    result.sort();
    result
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
    fn matching_is_case_insensitive_and_exact() {
        let filter = ParameterFilter::default();
        assert!(filter.matches_key("PASSWORD"));
        assert!(!filter.matches_key("customer_password"));
        assert!(!filter.matches_key("assignment"));
        assert!(!filter.matches_key("broken"));
    }

    #[test]
    fn additive_custom_key_and_opt_out() {
        let filter = ParameterFilter::new(&["pin".to_owned()], &["password".to_owned()]);
        let payload = json!({"password":"open","pin":"1234"});
        let out = filter.scrub_json(&payload);
        assert_eq!(out["password"], "open");
        assert_eq!(out["pin"], FILTERED_PLACEHOLDER);
    }

    #[test]
    fn empty_custom_key_does_not_scrub_everything() {
        let filter = ParameterFilter::new(&["".to_owned()], &[]);
        assert!(!filter.matches_key("email"));
        assert!(!filter.matches_key("anything"));
    }

    #[test]
    fn reports_opted_out_defaults() {
        let opts = vec![
            "PASSWORD".to_owned(),
            "missing".to_owned(),
            "apiKey".to_owned(),
        ];
        let normalized = normalized_opt_out_defaults(&opts);
        assert_eq!(normalized, vec!["apikey".to_owned(), "password".to_owned()]);
    }

    #[test]
    fn api_key_variants_match_after_normalization() {
        let filter = ParameterFilter::default();
        assert!(filter.matches_key("api_key"));
        assert!(filter.matches_key("apiKey"));
        assert!(filter.matches_key("apikey"));
        assert!(filter.matches_key("API-KEY"));
    }
}
