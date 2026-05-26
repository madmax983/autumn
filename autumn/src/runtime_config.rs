//! Runtime configuration store for live-tunable typed values.
//!
//! Provides a typed, validated, pluggable key-value store for operational
//! knobs that need to change without a redeploy — rate-limit ceilings,
//! timeouts, retry counts, support email, batch sizes, and similar tunables.
//!
//! # Quick start
//!
//! ```rust
//! use autumn_web::runtime_config::{
//!     ConfigKeySchema, ConfigRegistry, ConfigValue, ConfigValueType,
//!     InMemoryConfigStore, RuntimeConfigService,
//! };
//! use std::sync::Arc;
//!
//! // 1. Declare your keys with types and defaults.
//! let mut registry = ConfigRegistry::new();
//! registry.define(
//!     ConfigKeySchema::new("max_upload_mb", ConfigValueType::Int, ConfigValue::Int(50))
//!         .description("Maximum upload size in megabytes"),
//! ).unwrap();
//!
//! // 2. Pick a store (InMemoryConfigStore for tests; Postgres for production).
//! let store = Arc::new(InMemoryConfigStore::new());
//!
//! // 3. Build the service.
//! let svc = RuntimeConfigService::new(Arc::new(registry), store);
//!
//! // 4. Read (falls back to default when unset).
//! let mb = svc.get("max_upload_mb").unwrap();
//! assert_eq!(mb, ConfigValue::Int(50));
//!
//! // 5. Set a new value.
//! svc.set("max_upload_mb", "100", Some("ops")).unwrap();
//! assert_eq!(svc.get("max_upload_mb").unwrap(), ConfigValue::Int(100));
//! ```
//!
//! # Design
//!
//! - **Pluggable**: swap the [`ConfigStore`] trait for Redis, etcd, or a test double.
//! - **Typed**: the service layer parses and validates raw strings before writing.
//! - **Auditable**: every mutation records actor, old value, new value, and timestamp.
//! - **Schema-enforced**: unknown keys are rejected; type drift is caught on write.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

// ── Value types ───────────────────────────────────────────────────────────────

/// The type of a configuration key, used for parsing and validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueType {
    /// 64-bit signed integer.
    Int,
    /// 64-bit IEEE 754 float.
    Float,
    /// UTF-8 text.
    Text,
    /// Boolean (true/false).
    Bool,
    /// Duration expressed as whole seconds.
    DurationSecs,
    /// Arbitrary JSON value.
    Json,
}

impl ConfigValueType {
    /// Human-readable name for error messages.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Int => "i64",
            Self::Float => "f64",
            Self::Text => "String",
            Self::Bool => "bool",
            Self::DurationSecs => "Duration (seconds)",
            Self::Json => "JSON",
        }
    }
}

impl std::fmt::Display for ConfigValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A typed configuration value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConfigValue {
    /// 64-bit signed integer.
    Int(i64),
    /// 64-bit IEEE 754 float.
    Float(f64),
    /// UTF-8 text string.
    Text(String),
    /// Boolean.
    Bool(bool),
    /// Duration as whole seconds.
    DurationSecs(u64),
    /// Arbitrary JSON.
    Json(serde_json::Value),
}

impl ConfigValue {
    /// The type tag for this value.
    #[must_use]
    pub fn value_type(&self) -> ConfigValueType {
        match self {
            Self::Int(_) => ConfigValueType::Int,
            Self::Float(_) => ConfigValueType::Float,
            Self::Text(_) => ConfigValueType::Text,
            Self::Bool(_) => ConfigValueType::Bool,
            Self::DurationSecs(_) => ConfigValueType::DurationSecs,
            Self::Json(_) => ConfigValueType::Json,
        }
    }

    /// Parse a raw string into a typed value based on the expected type.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string when the raw input cannot be
    /// parsed as the expected type.
    pub fn parse_as(raw: &str, value_type: ConfigValueType) -> Result<Self, String> {
        match value_type {
            ConfigValueType::Int => raw
                .trim()
                .parse::<i64>()
                .map(ConfigValue::Int)
                .map_err(|_| {
                    format!("expected {}, got '{raw}'", ConfigValueType::Int.as_str())
                }),
            ConfigValueType::Float => raw
                .trim()
                .parse::<f64>()
                .map(ConfigValue::Float)
                .map_err(|_| {
                    format!("expected {}, got '{raw}'", ConfigValueType::Float.as_str())
                }),
            ConfigValueType::Text => Ok(ConfigValue::Text(raw.to_owned())),
            ConfigValueType::Bool => match raw.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" | "on" => Ok(ConfigValue::Bool(true)),
                "false" | "no" | "0" | "off" => Ok(ConfigValue::Bool(false)),
                _ => Err(format!(
                    "expected bool (true/false/yes/no/1/0/on/off), got '{raw}'"
                )),
            },
            ConfigValueType::DurationSecs => raw
                .trim()
                .parse::<u64>()
                .map(ConfigValue::DurationSecs)
                .map_err(|_| {
                    format!(
                        "expected {} (non-negative integer seconds), got '{raw}'",
                        ConfigValueType::DurationSecs.as_str()
                    )
                }),
            ConfigValueType::Json => serde_json::from_str(raw)
                .map(ConfigValue::Json)
                .map_err(|e| {
                    format!(
                        "expected {}, got '{raw}': {e}",
                        ConfigValueType::Json.as_str()
                    )
                }),
        }
    }

    /// Serialize this value to a canonical string for storage.
    #[must_use]
    pub fn to_raw(&self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Text(v) => v.clone(),
            Self::Bool(v) => v.to_string(),
            Self::DurationSecs(v) => v.to_string(),
            Self::Json(v) => v.to_string(),
        }
    }

    /// Returns the inner `i64` if this is [`ConfigValue::Int`].
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        if let Self::Int(v) = self { Some(*v) } else { None }
    }

    /// Returns the inner `f64` if this is [`ConfigValue::Float`].
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        if let Self::Float(v) = self { Some(*v) } else { None }
    }

    /// Returns the inner `&str` if this is [`ConfigValue::Text`].
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        if let Self::Text(v) = self { Some(v.as_str()) } else { None }
    }

    /// Returns the inner `bool` if this is [`ConfigValue::Bool`].
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(v) = self { Some(*v) } else { None }
    }

    /// Returns the duration in seconds if this is [`ConfigValue::DurationSecs`].
    #[must_use]
    pub fn as_duration_secs(&self) -> Option<u64> {
        if let Self::DurationSecs(v) = self { Some(*v) } else { None }
    }

    /// Returns the `std::time::Duration` if this is [`ConfigValue::DurationSecs`].
    #[must_use]
    pub fn as_duration(&self) -> Option<std::time::Duration> {
        self.as_duration_secs()
            .map(std::time::Duration::from_secs)
    }

    /// Returns the inner [`serde_json::Value`] if this is [`ConfigValue::Json`].
    #[must_use]
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        if let Self::Json(v) = self { Some(v) } else { None }
    }
}

impl std::fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_raw())
    }
}

// ── Validators ────────────────────────────────────────────────────────────────

/// A per-key validator applied before a write is accepted.
///
/// The service layer applies all declared validators in order. The first
/// rejection produces an error and the write is not propagated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigValidator {
    /// Inclusive integer range. Either bound may be omitted.
    IntRange {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// Inclusive float range. Either bound may be omitted.
    FloatRange {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    /// Whitelist of allowed string values (case-sensitive).
    AllowedValues(Vec<String>),
    /// POSIX-compatible regex pattern that the text value must fully match.
    Regex(String),
}

impl ConfigValidator {
    /// Apply this validator to `value`.
    ///
    /// Returns `Ok(())` if the value passes, or a human-readable error string.
    pub fn validate(&self, value: &ConfigValue) -> Result<(), String> {
        match self {
            Self::IntRange { min, max } => {
                let n = value
                    .as_int()
                    .ok_or_else(|| "IntRange validator applied to non-integer value".to_owned())?;
                if let Some(lo) = min {
                    if n < *lo {
                        return Err(format!("value {n} is below minimum {lo}"));
                    }
                }
                if let Some(hi) = max {
                    if n > *hi {
                        return Err(format!("value {n} exceeds maximum {hi}"));
                    }
                }
                Ok(())
            }
            Self::FloatRange { min, max } => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "FloatRange validator applied to non-float value".to_owned())?;
                if let Some(lo) = min {
                    if v < *lo {
                        return Err(format!("value {v} is below minimum {lo}"));
                    }
                }
                if let Some(hi) = max {
                    if v > *hi {
                        return Err(format!("value {v} exceeds maximum {hi}"));
                    }
                }
                Ok(())
            }
            Self::AllowedValues(allowed) => {
                let s = value
                    .as_text()
                    .ok_or_else(|| "AllowedValues validator applied to non-text value".to_owned())?;
                if allowed.iter().any(|a| a == s) {
                    Ok(())
                } else {
                    Err(format!(
                        "'{s}' is not an allowed value; expected one of: {}",
                        allowed.join(", ")
                    ))
                }
            }
            Self::Regex(pattern) => {
                let s = value
                    .as_text()
                    .ok_or_else(|| "Regex validator applied to non-text value".to_owned())?;
                // Simple deterministic regex check using the built-in approach
                // (no regex dep) — validate by trying to construct and match.
                if regex_matches(pattern, s) {
                    Ok(())
                } else {
                    Err(format!("'{s}' does not match required pattern '{pattern}'"))
                }
            }
        }
    }
}

/// Minimal anchored full-string regex matching.
///
/// Supports `^`, `$`, `.`, `*`, `+`, `?`, `[...]` (including ranges and `^`
/// negation), and literal characters. The engine is a recursive backtracking
/// NFA sufficient for config-validation patterns (email-like, digits, enums).
fn regex_matches(pattern: &str, text: &str) -> bool {
    let bytes = pattern.as_bytes();
    // Strip optional anchors — we always do a full-string match.
    let start = if bytes.first() == Some(&b'^') { 1 } else { 0 };
    let end = if bytes.last() == Some(&b'$') {
        bytes.len() - 1
    } else {
        bytes.len()
    };
    let pat = &bytes[start..end];
    re_match(pat, text.as_bytes())
}

/// Returns `true` when `pat` fully matches `text`.
fn re_match(pat: &[u8], text: &[u8]) -> bool {
    if pat.is_empty() {
        return text.is_empty();
    }

    // Extract the next atom (single char, `.`, or `[...]`).
    let (atom_len, atom) = re_next_atom(pat);
    let rest_pat = &pat[atom_len..];

    // Peek for a quantifier after the atom.
    let quantifier = rest_pat.first().copied();
    let (min, max, after_quant) = match quantifier {
        Some(b'*') => (0usize, usize::MAX, &rest_pat[1..]),
        Some(b'+') => (1, usize::MAX, &rest_pat[1..]),
        Some(b'?') => (0, 1, &rest_pat[1..]),
        _ => (1, 1, rest_pat),
    };

    // Greedy: count how many times the atom matches from the front.
    let mut matched_positions: Vec<usize> = vec![0];
    let mut pos = 0usize;
    let mut count = 0usize;
    while count < max && pos < text.len() && re_atom_matches(atom, text[pos]) {
        pos += 1;
        count += 1;
        matched_positions.push(pos);
    }

    if count < min {
        return false;
    }

    // Backtrack from greedy maximum down to minimum.
    for k in (min..=count).rev() {
        if re_match(after_quant, &text[matched_positions[k]..]) {
            return true;
        }
    }

    false
}

/// Extract the next atom from `pat`, returning `(atom_byte_length, atom_bytes)`.
fn re_next_atom(pat: &[u8]) -> (usize, &[u8]) {
    if pat.is_empty() {
        return (0, &[]);
    }
    if pat[0] == b'[' {
        // Find the closing `]`.
        if let Some(rel) = pat[1..].iter().position(|&b| b == b']') {
            let close = rel + 1; // index of `]` in `pat`
            return (close + 1, &pat[..=close]);
        }
    }
    // Escaped character or single character.
    if pat[0] == b'\\' && pat.len() > 1 {
        return (2, &pat[..2]);
    }
    (1, &pat[..1])
}

/// Returns `true` when `atom` matches byte `ch`.
fn re_atom_matches(atom: &[u8], ch: u8) -> bool {
    if atom.is_empty() {
        return false;
    }
    if atom[0] == b'[' {
        // Character class: strip brackets.
        let inner = if atom.len() >= 2 && atom[atom.len() - 1] == b']' {
            &atom[1..atom.len() - 1]
        } else {
            &atom[1..]
        };
        return re_class_matches(inner, ch);
    }
    if atom[0] == b'\\' && atom.len() > 1 {
        // Escaped literal: treat the second byte as a literal character.
        return atom[1] == ch;
    }
    match atom[0] {
        b'.' => true,
        c => c == ch,
    }
}

/// Match a character class body (between the `[` and `]`) against `ch`.
fn re_class_matches(inner: &[u8], ch: u8) -> bool {
    let (negate, body) = if inner.first() == Some(&b'^') {
        (true, &inner[1..])
    } else {
        (false, inner)
    };

    let mut i = 0usize;
    let mut matched = false;
    while i < body.len() {
        // Handle escape inside class.
        let (c, advance) = if body[i] == b'\\' && i + 1 < body.len() {
            (body[i + 1], 2)
        } else {
            (body[i], 1)
        };

        // Range: c-d.
        if i + advance + 1 < body.len() && body[i + advance] == b'-' {
            let (end_c, end_advance) =
                if body[i + advance + 1] == b'\\' && i + advance + 2 < body.len() {
                    (body[i + advance + 2], 2)
                } else {
                    (body[i + advance + 1], 1)
                };
            if ch >= c && ch <= end_c {
                matched = true;
            }
            i += advance + 1 + end_advance;
        } else {
            if c == ch {
                matched = true;
            }
            i += advance;
        }
    }

    if negate { !matched } else { matched }
}

// ── Schema ────────────────────────────────────────────────────────────────────

/// Schema declaration for a single runtime config key.
///
/// Build via [`ConfigKeySchema::new`] and add validators with the builder API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigKeySchema {
    pub name: String,
    pub value_type: ConfigValueType,
    pub default: ConfigValue,
    pub description: Option<String>,
    pub validators: Vec<ConfigValidator>,
}

impl ConfigKeySchema {
    /// Create a new schema entry.
    ///
    /// `default` must be of the correct `value_type` — this is checked in
    /// [`ConfigRegistry::define`].
    #[must_use]
    pub fn new(name: impl Into<String>, value_type: ConfigValueType, default: ConfigValue) -> Self {
        Self {
            name: name.into(),
            value_type,
            default,
            description: None,
            validators: Vec::new(),
        }
    }

    /// Set a human-readable description (shown in `autumn config list`).
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Attach a validator.
    ///
    /// Multiple validators are applied in order; the first rejection wins.
    #[must_use]
    pub fn validator(mut self, v: ConfigValidator) -> Self {
        self.validators.push(v);
        self
    }

    /// Validate a typed value against this key's schema.
    ///
    /// Checks that the value's type matches and all registered validators pass.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string on type mismatch or validator failure.
    pub fn validate(&self, value: &ConfigValue) -> Result<(), String> {
        if value.value_type() != self.value_type {
            return Err(format!(
                "key '{}': expected type {}, got {}",
                self.name,
                self.value_type,
                value.value_type()
            ));
        }
        for validator in &self.validators {
            validator.validate(value).map_err(|reason| {
                format!("key '{}': validation failed — {reason}", self.name)
            })?;
        }
        Ok(())
    }
}

/// Registry of all declared runtime config keys.
///
/// Build one at application start; wrap it in `Arc` and share it with the
/// [`RuntimeConfigService`].
#[derive(Debug, Default)]
pub struct ConfigRegistry {
    keys: HashMap<String, ConfigKeySchema>,
}

/// Error returned by [`ConfigRegistry::define`].
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// A key with the same name has already been registered.
    #[error("config key '{0}' is already registered")]
    DuplicateKey(String),
    /// The declared default's type does not match the declared `value_type`.
    #[error(
        "config key '{key}': default value type {default_type} does not match declared type {declared_type}"
    )]
    DefaultTypeMismatch {
        key: String,
        declared_type: ConfigValueType,
        default_type: ConfigValueType,
    },
    /// The key name is empty or contains disallowed characters (only `[a-z0-9_]` allowed).
    #[error("invalid config key name '{0}': must match [a-z][a-z0-9_]*")]
    InvalidKeyName(String),
}

impl ConfigRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a config key.
    ///
    /// # Errors
    ///
    /// - [`RegistryError::DuplicateKey`] if a key with the same name exists.
    /// - [`RegistryError::DefaultTypeMismatch`] if the default's type ≠ `value_type`.
    /// - [`RegistryError::InvalidKeyName`] if the name contains disallowed characters.
    pub fn define(&mut self, schema: ConfigKeySchema) -> Result<(), RegistryError> {
        if !is_valid_key_name(&schema.name) {
            return Err(RegistryError::InvalidKeyName(schema.name));
        }
        if self.keys.contains_key(&schema.name) {
            return Err(RegistryError::DuplicateKey(schema.name));
        }
        if schema.default.value_type() != schema.value_type {
            return Err(RegistryError::DefaultTypeMismatch {
                key: schema.name,
                declared_type: schema.value_type,
                default_type: schema.default.value_type(),
            });
        }
        self.keys.insert(schema.name.clone(), schema);
        Ok(())
    }

    /// Look up a key's schema by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ConfigKeySchema> {
        self.keys.get(name)
    }

    /// Iterate over all registered keys in undefined order.
    pub fn iter(&self) -> impl Iterator<Item = &ConfigKeySchema> {
        self.keys.values()
    }

    /// Number of registered keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns `true` if no keys are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

fn is_valid_key_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// ── Audit trail ───────────────────────────────────────────────────────────────

/// A single mutation recorded in the config change log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeRecord {
    /// The key that was changed.
    pub key: String,
    /// The value before the change (`None` = was using the default / not set).
    pub old_value: Option<ConfigValue>,
    /// The value after the change (`None` = reverted to default / unset).
    pub new_value: Option<ConfigValue>,
    /// Actor identifier supplied by the caller (username, principal, "cli", etc.).
    pub actor: Option<String>,
    /// Wall-clock time of the change in seconds since UNIX epoch.
    pub timestamp_secs: u64,
}

impl ConfigChangeRecord {
    fn now(
        key: &str,
        old_value: Option<ConfigValue>,
        new_value: Option<ConfigValue>,
        actor: Option<&str>,
    ) -> Self {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            key: key.to_owned(),
            old_value,
            new_value,
            actor: actor.map(str::to_owned),
            timestamp_secs,
        }
    }
}

// ── ConfigStore trait ─────────────────────────────────────────────────────────

/// Error from a [`ConfigStore`] backend.
#[derive(Debug, thiserror::Error)]
pub enum ConfigStoreError {
    /// The backend reported an I/O or connection failure.
    #[error("config store backend error: {0}")]
    Backend(String),
}

/// Pluggable storage backend for runtime config.
///
/// Implementors are responsible only for persistence and history. Type
/// validation is performed by the [`RuntimeConfigService`] layer before any
/// store method is called.
pub trait ConfigStore: Send + Sync + 'static {
    /// Return the stored raw string for `key`, or `None` if unset.
    fn get_raw(&self, key: &str) -> Option<String>;

    /// Persist a new raw string value for `key`.
    ///
    /// The old value (if any) is recorded in the change history.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigStoreError`] on backend failure.
    fn set_raw(
        &self,
        key: &str,
        old_raw: Option<String>,
        new_raw: String,
        actor: Option<&str>,
    ) -> Result<(), ConfigStoreError>;

    /// Remove the stored override for `key`, reverting to the schema default.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigStoreError`] on backend failure.
    fn unset_raw(
        &self,
        key: &str,
        old_raw: Option<String>,
        actor: Option<&str>,
    ) -> Result<(), ConfigStoreError>;

    /// Return all keys that have an active override (i.e. not using the default).
    fn list_overrides(&self) -> Vec<(String, String)>;

    /// Return the most recent `limit` change records for `key`.
    fn history(&self, key: &str, limit: usize) -> Vec<ConfigChangeRecord>;
}

// ── InMemoryConfigStore ───────────────────────────────────────────────────────

/// A thread-safe in-memory [`ConfigStore`] suitable for tests and dev mode.
///
/// State is NOT shared across processes or replicas. For production use the
/// Postgres-backed store from `autumn_web::runtime_config::pg`.
#[derive(Debug, Default)]
pub struct InMemoryConfigStore {
    values: RwLock<HashMap<String, String>>,
    history: RwLock<HashMap<String, Vec<ConfigChangeRecord>>>,
}

impl InMemoryConfigStore {
    /// Create an empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ConfigStore for InMemoryConfigStore {
    fn get_raw(&self, key: &str) -> Option<String> {
        self.values.read().unwrap().get(key).cloned()
    }

    fn set_raw(
        &self,
        key: &str,
        old_raw: Option<String>,
        new_raw: String,
        actor: Option<&str>,
    ) -> Result<(), ConfigStoreError> {
        let old_value = old_raw.map(|s| ConfigValue::Text(s));
        let new_value = Some(ConfigValue::Text(new_raw.clone()));
        let record = ConfigChangeRecord::now(key, old_value, new_value, actor);
        self.values
            .write()
            .unwrap()
            .insert(key.to_owned(), new_raw);
        self.history
            .write()
            .unwrap()
            .entry(key.to_owned())
            .or_default()
            .push(record);
        Ok(())
    }

    fn unset_raw(
        &self,
        key: &str,
        old_raw: Option<String>,
        actor: Option<&str>,
    ) -> Result<(), ConfigStoreError> {
        let old_value = old_raw.map(|s| ConfigValue::Text(s));
        let record = ConfigChangeRecord::now(key, old_value, None, actor);
        self.values.write().unwrap().remove(key);
        self.history
            .write()
            .unwrap()
            .entry(key.to_owned())
            .or_default()
            .push(record);
        Ok(())
    }

    fn list_overrides(&self) -> Vec<(String, String)> {
        let guard = self.values.read().unwrap();
        let mut pairs: Vec<(String, String)> = guard
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    fn history(&self, key: &str, limit: usize) -> Vec<ConfigChangeRecord> {
        let guard = self.history.read().unwrap();
        guard
            .get(key)
            .map(|records| {
                records
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── Service errors ────────────────────────────────────────────────────────────

/// Error from [`RuntimeConfigService`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A key was referenced that was not declared in the registry.
    #[error("unknown config key '{0}'; declare it with ConfigRegistry::define")]
    UnknownKey(String),

    /// The raw string could not be parsed as the key's declared type.
    #[error("config key '{key}': type error — {reason}")]
    TypeMismatch { key: String, reason: String },

    /// A declared validator rejected the value.
    #[error("config key '{key}': validation failed — {reason}")]
    ValidationFailed { key: String, reason: String },

    /// The backing store returned an error.
    #[error("config store error: {0}")]
    Store(#[from] ConfigStoreError),
}

// ── RuntimeConfigService ──────────────────────────────────────────────────────

/// A snapshot of a single config key: schema defaults + current override.
#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub name: String,
    pub value_type: ConfigValueType,
    pub current: ConfigValue,
    pub default: ConfigValue,
    pub is_overridden: bool,
    pub description: Option<String>,
}

/// The main runtime configuration service.
///
/// Wraps a [`ConfigRegistry`] (for schema and defaults) and a [`ConfigStore`]
/// (for persistence), providing a typed, validated API for reading and writing
/// config values.
#[derive(Clone)]
pub struct RuntimeConfigService {
    registry: Arc<ConfigRegistry>,
    store: Arc<dyn ConfigStore>,
}

impl std::fmt::Debug for RuntimeConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfigService")
            .field("registry_keys", &self.registry.len())
            .finish_non_exhaustive()
    }
}

impl RuntimeConfigService {
    /// Create a new service from a registry and a store.
    #[must_use]
    pub fn new(registry: Arc<ConfigRegistry>, store: Arc<dyn ConfigStore>) -> Self {
        Self { registry, store }
    }

    /// Read the current value for `key`, falling back to the schema default.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::UnknownKey`] if the key is not in the registry.
    /// - [`ConfigError::TypeMismatch`] if the stored raw value cannot be parsed
    ///   (indicates store corruption — the default is NOT automatically applied
    ///   to prevent silently masking a corrupt value).
    pub fn get(&self, key: &str) -> Result<ConfigValue, ConfigError> {
        let schema = self
            .registry
            .get(key)
            .ok_or_else(|| ConfigError::UnknownKey(key.to_owned()))?;

        match self.store.get_raw(key) {
            Some(raw) => ConfigValue::parse_as(&raw, schema.value_type).map_err(|reason| {
                ConfigError::TypeMismatch {
                    key: key.to_owned(),
                    reason,
                }
            }),
            None => Ok(schema.default.clone()),
        }
    }

    /// Set `key` to the parsed and validated form of `raw_value`.
    ///
    /// `actor` is stored in the change history (e.g. `"ops@example.com"` or `"cli"`).
    ///
    /// # Errors
    ///
    /// - [`ConfigError::UnknownKey`] for unregistered keys.
    /// - [`ConfigError::TypeMismatch`] if `raw_value` cannot be parsed as the declared type.
    /// - [`ConfigError::ValidationFailed`] if a declared validator rejects the value.
    /// - [`ConfigError::Store`] on backend failure.
    pub fn set(&self, key: &str, raw_value: &str, actor: Option<&str>) -> Result<(), ConfigError> {
        let schema = self
            .registry
            .get(key)
            .ok_or_else(|| ConfigError::UnknownKey(key.to_owned()))?;

        let typed =
            ConfigValue::parse_as(raw_value, schema.value_type).map_err(|reason| {
                ConfigError::TypeMismatch {
                    key: key.to_owned(),
                    reason,
                }
            })?;

        schema.validate(&typed).map_err(|reason| {
            ConfigError::ValidationFailed {
                key: key.to_owned(),
                reason,
            }
        })?;

        let old_raw = self.store.get_raw(key);
        self.store
            .set_raw(key, old_raw, typed.to_raw(), actor)?;
        Ok(())
    }

    /// Revert `key` to its schema default by removing the stored override.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::UnknownKey`] for unregistered keys.
    /// - [`ConfigError::Store`] on backend failure.
    pub fn unset(&self, key: &str, actor: Option<&str>) -> Result<(), ConfigError> {
        self.registry
            .get(key)
            .ok_or_else(|| ConfigError::UnknownKey(key.to_owned()))?;
        let old_raw = self.store.get_raw(key);
        self.store.unset_raw(key, old_raw, actor)?;
        Ok(())
    }

    /// Return all keys with their current values and metadata.
    ///
    /// Keys are sorted alphabetically. Keys with no stored override show the
    /// schema default as `current`.
    #[must_use]
    pub fn list(&self) -> Vec<ConfigEntry> {
        let overrides: HashMap<String, String> = self
            .store
            .list_overrides()
            .into_iter()
            .collect();

        let mut entries: Vec<ConfigEntry> = self
            .registry
            .iter()
            .map(|schema| {
                let (current, is_overridden) = if let Some(raw) = overrides.get(&schema.name) {
                    let parsed = ConfigValue::parse_as(raw, schema.value_type)
                        .unwrap_or_else(|_| schema.default.clone());
                    (parsed, true)
                } else {
                    (schema.default.clone(), false)
                };
                ConfigEntry {
                    name: schema.name.clone(),
                    value_type: schema.value_type,
                    current,
                    default: schema.default.clone(),
                    is_overridden,
                    description: schema.description.clone(),
                }
            })
            .collect();

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// Return the most recent `limit` change records for `key`.
    ///
    /// Returns an empty vec for unknown keys (no error, just no history).
    #[must_use]
    pub fn history(&self, key: &str, limit: usize) -> Vec<ConfigChangeRecord> {
        self.store.history(key, limit)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_registry() -> ConfigRegistry {
        let mut r = ConfigRegistry::new();
        r.define(
            ConfigKeySchema::new("max_upload_mb", ConfigValueType::Int, ConfigValue::Int(50))
                .description("Max upload size in MB"),
        )
        .unwrap();
        r.define(ConfigKeySchema::new(
            "support_email",
            ConfigValueType::Text,
            ConfigValue::Text("support@example.com".to_owned()),
        ))
        .unwrap();
        r.define(ConfigKeySchema::new(
            "rate_limit_rps",
            ConfigValueType::Float,
            ConfigValue::Float(100.0),
        ))
        .unwrap();
        r.define(ConfigKeySchema::new(
            "maintenance_mode",
            ConfigValueType::Bool,
            ConfigValue::Bool(false),
        ))
        .unwrap();
        r.define(ConfigKeySchema::new(
            "cache_ttl",
            ConfigValueType::DurationSecs,
            ConfigValue::DurationSecs(300),
        ))
        .unwrap();
        r.define(ConfigKeySchema::new(
            "feature_flags",
            ConfigValueType::Json,
            ConfigValue::Json(serde_json::Value::Null),
        ))
        .unwrap();
        r
    }

    fn make_svc() -> RuntimeConfigService {
        let registry = Arc::new(make_registry());
        let store = Arc::new(InMemoryConfigStore::new());
        RuntimeConfigService::new(registry, store)
    }

    // ── ConfigValueType ───────────────────────────────────────────────────────

    #[test]
    fn value_type_as_str_matches_canonical_names() {
        assert_eq!(ConfigValueType::Int.as_str(), "i64");
        assert_eq!(ConfigValueType::Float.as_str(), "f64");
        assert_eq!(ConfigValueType::Text.as_str(), "String");
        assert_eq!(ConfigValueType::Bool.as_str(), "bool");
        assert_eq!(ConfigValueType::DurationSecs.as_str(), "Duration (seconds)");
        assert_eq!(ConfigValueType::Json.as_str(), "JSON");
    }

    // ── ConfigValue::parse_as ─────────────────────────────────────────────────

    #[test]
    fn parse_int_from_valid_string() {
        assert_eq!(
            ConfigValue::parse_as("42", ConfigValueType::Int).unwrap(),
            ConfigValue::Int(42)
        );
    }

    #[test]
    fn parse_negative_int() {
        assert_eq!(
            ConfigValue::parse_as("-100", ConfigValueType::Int).unwrap(),
            ConfigValue::Int(-100)
        );
    }

    #[test]
    fn parse_int_from_invalid_string_returns_error() {
        let err = ConfigValue::parse_as("foo", ConfigValueType::Int).unwrap_err();
        assert!(
            err.contains("i64"),
            "error should mention expected type: {err}"
        );
        assert!(err.contains("foo"), "error should echo the input: {err}");
    }

    #[test]
    fn parse_float_from_valid_string() {
        let v = ConfigValue::parse_as("3.14", ConfigValueType::Float).unwrap();
        assert_eq!(v, ConfigValue::Float(3.14));
    }

    #[test]
    fn parse_float_from_integer_string() {
        let v = ConfigValue::parse_as("10", ConfigValueType::Float).unwrap();
        assert_eq!(v, ConfigValue::Float(10.0));
    }

    #[test]
    fn parse_float_from_invalid_string_returns_error() {
        let err = ConfigValue::parse_as("abc", ConfigValueType::Float).unwrap_err();
        assert!(err.contains("f64"), "should mention type: {err}");
    }

    #[test]
    fn parse_text_always_succeeds() {
        let v = ConfigValue::parse_as("anything goes!", ConfigValueType::Text).unwrap();
        assert_eq!(v, ConfigValue::Text("anything goes!".to_owned()));
    }

    #[test]
    fn parse_bool_true_variants() {
        for raw in ["true", "yes", "1", "on", "True", "YES", "ON"] {
            let v = ConfigValue::parse_as(raw, ConfigValueType::Bool)
                .unwrap_or_else(|e| panic!("'{raw}' should parse as bool: {e}"));
            assert_eq!(v, ConfigValue::Bool(true), "'{raw}' should be true");
        }
    }

    #[test]
    fn parse_bool_false_variants() {
        for raw in ["false", "no", "0", "off", "False", "NO", "OFF"] {
            let v = ConfigValue::parse_as(raw, ConfigValueType::Bool)
                .unwrap_or_else(|e| panic!("'{raw}' should parse as bool: {e}"));
            assert_eq!(v, ConfigValue::Bool(false), "'{raw}' should be false");
        }
    }

    #[test]
    fn parse_bool_from_invalid_string_returns_error() {
        let err = ConfigValue::parse_as("maybe", ConfigValueType::Bool).unwrap_err();
        assert!(err.contains("bool"), "should mention type: {err}");
        assert!(err.contains("maybe"), "should echo input: {err}");
    }

    #[test]
    fn parse_duration_from_valid_seconds() {
        let v = ConfigValue::parse_as("3600", ConfigValueType::DurationSecs).unwrap();
        assert_eq!(v, ConfigValue::DurationSecs(3600));
    }

    #[test]
    fn parse_duration_from_zero() {
        let v = ConfigValue::parse_as("0", ConfigValueType::DurationSecs).unwrap();
        assert_eq!(v, ConfigValue::DurationSecs(0));
    }

    #[test]
    fn parse_duration_from_invalid_string_returns_error() {
        let err = ConfigValue::parse_as("2h", ConfigValueType::DurationSecs).unwrap_err();
        assert!(err.contains("Duration"), "should mention type: {err}");
    }

    #[test]
    fn parse_json_from_valid_json_string() {
        let v = ConfigValue::parse_as(r#"{"key":"val"}"#, ConfigValueType::Json).unwrap();
        assert_eq!(
            v,
            ConfigValue::Json(serde_json::json!({"key": "val"}))
        );
    }

    #[test]
    fn parse_json_from_invalid_json_returns_error() {
        let err = ConfigValue::parse_as("{not json}", ConfigValueType::Json).unwrap_err();
        assert!(err.contains("JSON"), "should mention type: {err}");
    }

    // ── ConfigValue accessors ─────────────────────────────────────────────────

    #[test]
    fn as_int_returns_some_for_int() {
        assert_eq!(ConfigValue::Int(42).as_int(), Some(42));
    }

    #[test]
    fn as_int_returns_none_for_non_int() {
        assert!(ConfigValue::Text("x".to_owned()).as_int().is_none());
    }

    #[test]
    fn as_bool_returns_some_for_bool() {
        assert_eq!(ConfigValue::Bool(true).as_bool(), Some(true));
    }

    #[test]
    fn as_text_returns_some_for_text() {
        assert_eq!(
            ConfigValue::Text("hi".to_owned()).as_text(),
            Some("hi")
        );
    }

    #[test]
    fn as_duration_converts_secs_to_std_duration() {
        let d = ConfigValue::DurationSecs(60).as_duration().unwrap();
        assert_eq!(d, std::time::Duration::from_secs(60));
    }

    #[test]
    fn to_raw_round_trips_int() {
        let v = ConfigValue::Int(99);
        let raw = v.to_raw();
        assert_eq!(ConfigValue::parse_as(&raw, ConfigValueType::Int).unwrap(), v);
    }

    #[test]
    fn to_raw_round_trips_bool() {
        let v = ConfigValue::Bool(true);
        assert_eq!(v.to_raw(), "true");
    }

    // ── ConfigValidator ───────────────────────────────────────────────────────

    #[test]
    fn int_range_accepts_value_within_bounds() {
        let v = ConfigValidator::IntRange {
            min: Some(1),
            max: Some(100),
        };
        v.validate(&ConfigValue::Int(50)).unwrap();
    }

    #[test]
    fn int_range_rejects_value_below_min() {
        let v = ConfigValidator::IntRange {
            min: Some(1),
            max: Some(100),
        };
        let err = v.validate(&ConfigValue::Int(0)).unwrap_err();
        assert!(
            err.contains("below minimum"),
            "should mention below minimum: {err}"
        );
    }

    #[test]
    fn int_range_rejects_value_above_max() {
        let v = ConfigValidator::IntRange {
            min: Some(1),
            max: Some(100),
        };
        let err = v.validate(&ConfigValue::Int(101)).unwrap_err();
        assert!(
            err.contains("exceeds maximum"),
            "should mention exceeds maximum: {err}"
        );
    }

    #[test]
    fn int_range_with_no_bounds_accepts_any_int() {
        let v = ConfigValidator::IntRange {
            min: None,
            max: None,
        };
        v.validate(&ConfigValue::Int(i64::MAX)).unwrap();
        v.validate(&ConfigValue::Int(i64::MIN)).unwrap();
    }

    #[test]
    fn float_range_accepts_value_within_bounds() {
        let v = ConfigValidator::FloatRange {
            min: Some(0.0),
            max: Some(1.0),
        };
        v.validate(&ConfigValue::Float(0.5)).unwrap();
    }

    #[test]
    fn float_range_rejects_value_out_of_bounds() {
        let v = ConfigValidator::FloatRange {
            min: Some(0.0),
            max: Some(1.0),
        };
        v.validate(&ConfigValue::Float(1.5)).unwrap_err();
    }

    #[test]
    fn allowed_values_accepts_matching_value() {
        let v = ConfigValidator::AllowedValues(vec![
            "draft".to_owned(),
            "published".to_owned(),
            "archived".to_owned(),
        ]);
        v.validate(&ConfigValue::Text("published".to_owned()))
            .unwrap();
    }

    #[test]
    fn allowed_values_rejects_non_matching_value() {
        let v = ConfigValidator::AllowedValues(vec!["a".to_owned(), "b".to_owned()]);
        let err = v
            .validate(&ConfigValue::Text("c".to_owned()))
            .unwrap_err();
        assert!(
            err.contains("not an allowed value"),
            "should mention not allowed: {err}"
        );
        assert!(err.contains("a"), "should list allowed values: {err}");
    }

    #[test]
    fn regex_validator_accepts_matching_value() {
        // Simple email-like pattern: user@host
        let v = ConfigValidator::Regex("[a-z0-9]+@[a-z0-9.]+".to_owned());
        v.validate(&ConfigValue::Text("ops@example.com".to_owned()))
            .unwrap();
    }

    #[test]
    fn regex_validator_rejects_non_matching_value() {
        let v = ConfigValidator::Regex("[0-9]+".to_owned());
        let err = v
            .validate(&ConfigValue::Text("not-a-number".to_owned()))
            .unwrap_err();
        assert!(err.contains("does not match"), "should say: {err}");
    }

    // ── ConfigKeySchema ───────────────────────────────────────────────────────

    #[test]
    fn schema_validate_passes_correct_type() {
        let schema = ConfigKeySchema::new("x", ConfigValueType::Int, ConfigValue::Int(1));
        schema.validate(&ConfigValue::Int(42)).unwrap();
    }

    #[test]
    fn schema_validate_rejects_wrong_type() {
        let schema = ConfigKeySchema::new("x", ConfigValueType::Int, ConfigValue::Int(1));
        let err = schema
            .validate(&ConfigValue::Text("hi".to_owned()))
            .unwrap_err();
        assert!(
            err.contains("expected type"),
            "should mention type mismatch: {err}"
        );
    }

    #[test]
    fn schema_validate_runs_attached_validators() {
        let schema = ConfigKeySchema::new("x", ConfigValueType::Int, ConfigValue::Int(5))
            .validator(ConfigValidator::IntRange {
                min: Some(1),
                max: Some(10),
            });
        schema.validate(&ConfigValue::Int(5)).unwrap();
        let err = schema.validate(&ConfigValue::Int(99)).unwrap_err();
        assert!(err.contains("exceeds maximum"), "{err}");
    }

    // ── ConfigRegistry ────────────────────────────────────────────────────────

    #[test]
    fn registry_define_and_lookup() {
        let mut r = ConfigRegistry::new();
        r.define(ConfigKeySchema::new(
            "timeout_secs",
            ConfigValueType::DurationSecs,
            ConfigValue::DurationSecs(30),
        ))
        .unwrap();
        assert!(r.get("timeout_secs").is_some());
        assert!(r.get("nonexistent").is_none());
    }

    #[test]
    fn registry_rejects_duplicate_key() {
        let mut r = ConfigRegistry::new();
        r.define(ConfigKeySchema::new(
            "key",
            ConfigValueType::Bool,
            ConfigValue::Bool(false),
        ))
        .unwrap();
        let err = r
            .define(ConfigKeySchema::new(
                "key",
                ConfigValueType::Bool,
                ConfigValue::Bool(true),
            ))
            .unwrap_err();
        assert!(
            matches!(err, RegistryError::DuplicateKey(_)),
            "expected DuplicateKey, got {err:?}"
        );
    }

    #[test]
    fn registry_rejects_default_type_mismatch() {
        let mut r = ConfigRegistry::new();
        let err = r
            .define(ConfigKeySchema::new(
                "key",
                ConfigValueType::Int,
                ConfigValue::Text("not an int".to_owned()),
            ))
            .unwrap_err();
        assert!(
            matches!(err, RegistryError::DefaultTypeMismatch { .. }),
            "expected DefaultTypeMismatch, got {err:?}"
        );
    }

    #[test]
    fn registry_rejects_invalid_key_name_starting_with_digit() {
        let mut r = ConfigRegistry::new();
        let err = r
            .define(ConfigKeySchema::new(
                "1invalid",
                ConfigValueType::Int,
                ConfigValue::Int(0),
            ))
            .unwrap_err();
        assert!(
            matches!(err, RegistryError::InvalidKeyName(_)),
            "expected InvalidKeyName"
        );
    }

    #[test]
    fn registry_rejects_empty_key_name() {
        let mut r = ConfigRegistry::new();
        let err = r
            .define(ConfigKeySchema::new("", ConfigValueType::Int, ConfigValue::Int(0)))
            .unwrap_err();
        assert!(matches!(err, RegistryError::InvalidKeyName(_)));
    }

    #[test]
    fn registry_accepts_lowercase_with_underscores_and_digits() {
        let mut r = ConfigRegistry::new();
        r.define(ConfigKeySchema::new(
            "max_retry_count2",
            ConfigValueType::Int,
            ConfigValue::Int(3),
        ))
        .unwrap();
        assert!(r.get("max_retry_count2").is_some());
    }

    #[test]
    fn registry_len_and_is_empty() {
        let r = ConfigRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        let r = make_registry();
        assert!(!r.is_empty());
        assert_eq!(r.len(), 6);
    }

    // ── InMemoryConfigStore ───────────────────────────────────────────────────

    #[test]
    fn in_memory_store_get_raw_returns_none_when_unset() {
        let store = InMemoryConfigStore::new();
        assert!(store.get_raw("anything").is_none());
    }

    #[test]
    fn in_memory_store_set_and_get_raw_roundtrip() {
        let store = InMemoryConfigStore::new();
        store.set_raw("key", None, "42".to_owned(), None).unwrap();
        assert_eq!(store.get_raw("key").as_deref(), Some("42"));
    }

    #[test]
    fn in_memory_store_unset_removes_value() {
        let store = InMemoryConfigStore::new();
        store
            .set_raw("key", None, "hello".to_owned(), None)
            .unwrap();
        store.unset_raw("key", Some("hello".to_owned()), None).unwrap();
        assert!(store.get_raw("key").is_none());
    }

    #[test]
    fn in_memory_store_list_overrides_returns_sorted_pairs() {
        let store = InMemoryConfigStore::new();
        store.set_raw("zzz", None, "1".to_owned(), None).unwrap();
        store.set_raw("aaa", None, "2".to_owned(), None).unwrap();
        let pairs = store.list_overrides();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "aaa");
        assert_eq!(pairs[1].0, "zzz");
    }

    #[test]
    fn in_memory_store_history_records_changes() {
        let store = InMemoryConfigStore::new();
        store.set_raw("key", None, "10".to_owned(), Some("alice")).unwrap();
        store
            .set_raw("key", Some("10".to_owned()), "20".to_owned(), Some("bob"))
            .unwrap();
        let history = store.history("key", 10);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].actor.as_deref(), Some("bob"));
        assert_eq!(history[1].actor.as_deref(), Some("alice"));
    }

    #[test]
    fn in_memory_store_history_limit_is_respected() {
        let store = InMemoryConfigStore::new();
        for i in 0..10 {
            store
                .set_raw("key", Some(i.to_string()), (i + 1).to_string(), None)
                .unwrap();
        }
        let history = store.history("key", 3);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn in_memory_store_history_empty_for_unknown_key() {
        let store = InMemoryConfigStore::new();
        let history = store.history("nonexistent", 10);
        assert!(history.is_empty());
    }

    // ── RuntimeConfigService ──────────────────────────────────────────────────

    #[test]
    fn service_get_returns_default_when_unset() {
        let svc = make_svc();
        let v = svc.get("max_upload_mb").unwrap();
        assert_eq!(v, ConfigValue::Int(50));
    }

    #[test]
    fn service_get_returns_error_for_unknown_key() {
        let svc = make_svc();
        let err = svc.get("no_such_key").unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey(_)),
            "expected UnknownKey, got {err}"
        );
    }

    #[test]
    fn service_set_then_get_returns_new_value() {
        let svc = make_svc();
        svc.set("max_upload_mb", "200", Some("ops")).unwrap();
        assert_eq!(svc.get("max_upload_mb").unwrap(), ConfigValue::Int(200));
    }

    #[test]
    fn service_set_rejects_unknown_key() {
        let svc = make_svc();
        let err = svc.set("no_such_key", "42", None).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey(_)));
    }

    #[test]
    fn service_set_rejects_type_mismatch() {
        let svc = make_svc();
        let err = svc.set("max_upload_mb", "not_an_int", None).unwrap_err();
        assert!(
            matches!(err, ConfigError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("max_upload_mb"), "should name the key: {msg}");
    }

    #[test]
    fn service_set_rejects_value_failing_validator() {
        let mut registry = ConfigRegistry::new();
        registry
            .define(
                ConfigKeySchema::new("threads", ConfigValueType::Int, ConfigValue::Int(4))
                    .validator(ConfigValidator::IntRange {
                        min: Some(1),
                        max: Some(64),
                    }),
            )
            .unwrap();
        let svc = RuntimeConfigService::new(
            Arc::new(registry),
            Arc::new(InMemoryConfigStore::new()),
        );
        let err = svc.set("threads", "0", None).unwrap_err();
        assert!(
            matches!(err, ConfigError::ValidationFailed { .. }),
            "expected ValidationFailed, got {err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("threads"), "should name the key: {msg}");
    }

    #[test]
    fn service_unset_reverts_to_default() {
        let svc = make_svc();
        svc.set("max_upload_mb", "200", None).unwrap();
        svc.unset("max_upload_mb", None).unwrap();
        assert_eq!(svc.get("max_upload_mb").unwrap(), ConfigValue::Int(50));
    }

    #[test]
    fn service_unset_rejects_unknown_key() {
        let svc = make_svc();
        let err = svc.unset("no_such_key", None).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey(_)));
    }

    #[test]
    fn service_list_returns_all_keys_sorted() {
        let svc = make_svc();
        let entries = svc.list();
        assert_eq!(entries.len(), 6);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "entries should be sorted alphabetically");
    }

    #[test]
    fn service_list_marks_overridden_keys() {
        let svc = make_svc();
        svc.set("max_upload_mb", "100", None).unwrap();
        let entries = svc.list();
        let entry = entries.iter().find(|e| e.name == "max_upload_mb").unwrap();
        assert!(entry.is_overridden);
        assert_eq!(entry.current, ConfigValue::Int(100));
    }

    #[test]
    fn service_list_does_not_mark_unset_keys_as_overridden() {
        let svc = make_svc();
        let entries = svc.list();
        for entry in &entries {
            assert!(
                !entry.is_overridden,
                "key '{}' should not be marked overridden",
                entry.name
            );
        }
    }

    #[test]
    fn service_history_returns_changes_in_reverse_chronological_order() {
        let svc = make_svc();
        svc.set("max_upload_mb", "100", Some("alice")).unwrap();
        svc.set("max_upload_mb", "200", Some("bob")).unwrap();
        let history = svc.history("max_upload_mb", 10);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].actor.as_deref(), Some("bob"));
        assert_eq!(history[1].actor.as_deref(), Some("alice"));
    }

    #[test]
    fn service_history_returns_empty_for_unknown_key() {
        let svc = make_svc();
        let history = svc.history("nonexistent", 10);
        assert!(history.is_empty());
    }

    #[test]
    fn service_all_supported_types_roundtrip() {
        let mut registry = ConfigRegistry::new();
        registry
            .define(ConfigKeySchema::new(
                "i",
                ConfigValueType::Int,
                ConfigValue::Int(0),
            ))
            .unwrap();
        registry
            .define(ConfigKeySchema::new(
                "f",
                ConfigValueType::Float,
                ConfigValue::Float(0.0),
            ))
            .unwrap();
        registry
            .define(ConfigKeySchema::new(
                "t",
                ConfigValueType::Text,
                ConfigValue::Text(String::new()),
            ))
            .unwrap();
        registry
            .define(ConfigKeySchema::new(
                "b",
                ConfigValueType::Bool,
                ConfigValue::Bool(false),
            ))
            .unwrap();
        registry
            .define(ConfigKeySchema::new(
                "d",
                ConfigValueType::DurationSecs,
                ConfigValue::DurationSecs(0),
            ))
            .unwrap();
        registry
            .define(ConfigKeySchema::new(
                "j",
                ConfigValueType::Json,
                ConfigValue::Json(serde_json::Value::Null),
            ))
            .unwrap();
        let svc = RuntimeConfigService::new(
            Arc::new(registry),
            Arc::new(InMemoryConfigStore::new()),
        );
        svc.set("i", "7", None).unwrap();
        svc.set("f", "3.14", None).unwrap();
        svc.set("t", "hello", None).unwrap();
        svc.set("b", "true", None).unwrap();
        svc.set("d", "3600", None).unwrap();
        svc.set("j", r#"[1,2,3]"#, None).unwrap();

        assert_eq!(svc.get("i").unwrap(), ConfigValue::Int(7));
        assert_eq!(svc.get("f").unwrap(), ConfigValue::Float(3.14));
        assert_eq!(svc.get("t").unwrap(), ConfigValue::Text("hello".to_owned()));
        assert_eq!(svc.get("b").unwrap(), ConfigValue::Bool(true));
        assert_eq!(svc.get("d").unwrap(), ConfigValue::DurationSecs(3600));
        assert_eq!(
            svc.get("j").unwrap(),
            ConfigValue::Json(serde_json::json!([1, 2, 3]))
        );
    }

    #[test]
    fn service_set_does_not_update_on_validation_failure() {
        let mut registry = ConfigRegistry::new();
        registry
            .define(
                ConfigKeySchema::new("retries", ConfigValueType::Int, ConfigValue::Int(3))
                    .validator(ConfigValidator::IntRange {
                        min: Some(0),
                        max: Some(10),
                    }),
            )
            .unwrap();
        let svc = RuntimeConfigService::new(
            Arc::new(registry),
            Arc::new(InMemoryConfigStore::new()),
        );
        svc.set("retries", "5", None).unwrap();
        assert_eq!(svc.get("retries").unwrap(), ConfigValue::Int(5));
        // Failing write should not change the stored value.
        svc.set("retries", "999", None).unwrap_err();
        assert_eq!(
            svc.get("retries").unwrap(),
            ConfigValue::Int(5),
            "failed write must not persist"
        );
    }

    // ── regex_matches ─────────────────────────────────────────────────────────

    #[test]
    fn regex_matches_literal_text() {
        assert!(regex_matches("hello", "hello"));
        assert!(!regex_matches("hello", "world"));
    }

    #[test]
    fn regex_dot_matches_any_char() {
        assert!(regex_matches("h.llo", "hello"));
        assert!(regex_matches("h.llo", "hXllo"));
        assert!(!regex_matches("h.llo", "hllo"));
    }

    #[test]
    fn regex_star_matches_zero_or_more() {
        assert!(regex_matches("ab*c", "ac"));
        assert!(regex_matches("ab*c", "abc"));
        assert!(regex_matches("ab*c", "abbbc"));
    }

    #[test]
    fn regex_char_class_matches_digits() {
        assert!(regex_matches("[0-9]+", "42"));
        assert!(!regex_matches("[0-9]+", "abc"));
    }
}
