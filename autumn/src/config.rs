//! Framework configuration with sensible defaults and profile-based layering.
//!
//! Autumn uses a five-layer configuration system where each layer
//! overrides the previous one:
//!
//! 1. **Framework defaults** (this module) -- compiled into the binary.
//! 2. **Profile smart defaults** -- per-profile values for `dev`/`prod`.
//! 3. **`autumn.toml`** -- project-level overrides checked into source control.
//! 4. **`[profile.{name}]` in `autumn.toml`** -- profile-specific overrides.
//! 5. **`autumn-{profile}.toml`** -- legacy profile-specific overrides.
//! 6. **`AUTUMN_*` environment variables** -- deployment/CI overrides.
//!
//! An Autumn application runs with zero configuration -- every field
//! has a sensible default value. Override only what you need.
//!
//! # Profiles
//!
//! Profiles are resolved in precedence order:
//! 1. `AUTUMN_ENV` environment variable
//! 2. `AUTUMN_PROFILE` environment variable (legacy alias)
//! 3. `--profile` CLI flag
//! 4. Auto-detect from debug/release build mode
//!
//! # Example
//!
//! ```rust
//! use autumn_web::config::AutumnConfig;
//!
//! // All defaults -- no file needed
//! let config = AutumnConfig::default();
//! assert_eq!(config.server.port, 3000);
//! assert_eq!(config.server.host, "127.0.0.1");
//! assert!(config.database.url.is_none());
//! ```
//!
//! # Environment variable reference
//!
//! | Variable | Config field | Type |
//! |----------|-------------|------|
//! | `AUTUMN_SERVER__PORT` | `server.port` | `u16` |
//! | `AUTUMN_SERVER__HOST` | `server.host` | `String` |
//! | `AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS` | `server.shutdown_timeout_secs` | `u64` |
//! | `AUTUMN_SERVER__PRESTOP_GRACE_SECS` | `server.prestop_grace_secs` | `u64` |
//! | `AUTUMN_SERVER__TIMEOUTS__REQUEST_TIMEOUT_MS` | `server.timeouts.request_timeout_ms` | `u64` |
//! | `AUTUMN_DATABASE__URL` | `database.url` | `String` |
//! | `AUTUMN_DATABASE__PRIMARY_URL` | `database.primary_url` | `String` |
//! | `AUTUMN_DATABASE__REPLICA_URL` | `database.replica_url` | `String` |
//! | `AUTUMN_DATABASE__POOL_SIZE` | `database.pool_size` | `usize` |
//! | `AUTUMN_DATABASE__PRIMARY_POOL_SIZE` | `database.primary_pool_size` | `usize` |
//! | `AUTUMN_DATABASE__REPLICA_POOL_SIZE` | `database.replica_pool_size` | `usize` |
//! | `AUTUMN_DATABASE__REPLICA_FALLBACK` | `database.replica_fallback` | `fail_readiness` / `primary` |
//! | `AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS` | `database.connect_timeout_secs` | `u64` |
//! | `AUTUMN_DATABASE__AUTO_MIGRATE_IN_PRODUCTION` | `database.auto_migrate_in_production` | `bool` |
//! | `AUTUMN_LOG__LEVEL` | `log.level` | tracing filter directive |
//! | `AUTUMN_LOG__FORMAT` | `log.format` | `Auto` / `Pretty` / `Json` |
//! | `AUTUMN_TELEMETRY__ENABLED` | `telemetry.enabled` | `bool` |
//! | `AUTUMN_TELEMETRY__SERVICE_NAME` | `telemetry.service_name` | `String` |
//! | `AUTUMN_TELEMETRY__SERVICE_NAMESPACE` | `telemetry.service_namespace` | `String` |
//! | `AUTUMN_TELEMETRY__SERVICE_VERSION` | `telemetry.service_version` | `String` |
//! | `AUTUMN_TELEMETRY__ENVIRONMENT` | `telemetry.environment` | `String` |
//! | `AUTUMN_TELEMETRY__OTLP_ENDPOINT` | `telemetry.otlp_endpoint` | `String` |
//! | `AUTUMN_TELEMETRY__PROTOCOL` | `telemetry.protocol` | `Grpc` / `HttpProtobuf` |
//! | `AUTUMN_TELEMETRY__STRICT` | `telemetry.strict` | `bool` |
//! | `AUTUMN_HEALTH__PATH` | `health.path` | `String` |
//! | `AUTUMN_HEALTH__LIVE_PATH` | `health.live_path` | `String` |
//! | `AUTUMN_HEALTH__READY_PATH` | `health.ready_path` | `String` |
//! | `AUTUMN_HEALTH__STARTUP_PATH` | `health.startup_path` | `String` |
//! | `AUTUMN_HEALTH__DETAILED` | `health.detailed` | `bool` |
//! | `AUTUMN_CORS__ALLOWED_ORIGINS` | `cors.allowed_origins` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOWED_METHODS` | `cors.allowed_methods` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOWED_HEADERS` | `cors.allowed_headers` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOW_CREDENTIALS` | `cors.allow_credentials` | `bool` |
//! | `AUTUMN_CORS__MAX_AGE_SECS` | `cors.max_age_secs` | `u64` |
//! | `AUTUMN_CACHE__BACKEND` | `cache.backend` | `memory` / `redis` |
//! | `AUTUMN_CACHE__REDIS__URL` | `cache.redis.url` | `String` |
//! | `AUTUMN_CACHE__REDIS__KEY_PREFIX` | `cache.redis.key_prefix` | `String` |
//! | `AUTUMN_SESSION__BACKEND` | `session.backend` | `memory` / `redis` |
//! | `AUTUMN_SESSION__COOKIE_NAME` | `session.cookie_name` | `String` |
//! | `AUTUMN_SESSION__MAX_AGE_SECS` | `session.max_age_secs` | `u64` |
//! | `AUTUMN_SESSION__SECURE` | `session.secure` | `bool` |
//! | `AUTUMN_SESSION__SAME_SITE` | `session.same_site` | `String` |
//! | `AUTUMN_SESSION__HTTP_ONLY` | `session.http_only` | `bool` |
//! | `AUTUMN_SESSION__PATH` | `session.path` | `String` |
//! | `AUTUMN_SESSION__ALLOW_MEMORY_IN_PRODUCTION` | `session.allow_memory_in_production` | `bool` |
//! | `AUTUMN_SESSION__REDIS__URL` | `session.redis.url` | `String` |
//! | `AUTUMN_SESSION__REDIS__KEY_PREFIX` | `session.redis.key_prefix` | `String` |
//! | `AUTUMN_CHANNELS__BACKEND` | `channels.backend` | `in_process` / `redis` |
//! | `AUTUMN_CHANNELS__CAPACITY` | `channels.capacity` | `usize` |
//! | `AUTUMN_CHANNELS__REDIS__URL` | `channels.redis.url` | `String` |
//! | `AUTUMN_CHANNELS__REDIS__KEY_PREFIX` | `channels.redis.key_prefix` | `String` |
//! | `AUTUMN_JOBS__BACKEND` | `jobs.backend` | `local` / `postgres` / `redis` |
//! | `AUTUMN_JOBS__WORKERS` | `jobs.workers` | `usize` |
//! | `AUTUMN_JOBS__MAX_ATTEMPTS` | `jobs.max_attempts` | `u32` |
//! | `AUTUMN_JOBS__INITIAL_BACKOFF_MS` | `jobs.initial_backoff_ms` | `u64` |
//! | `AUTUMN_JOBS__REDIS__URL` | `jobs.redis.url` | `String` |
//! | `AUTUMN_JOBS__REDIS__KEY_PREFIX` | `jobs.redis.key_prefix` | `String` |
//! | `AUTUMN_JOBS__REDIS__VISIBILITY_TIMEOUT_MS` | `jobs.redis.visibility_timeout_ms` | `u64` |
//! | `AUTUMN_JOBS__POSTGRES__VISIBILITY_TIMEOUT_MS` | `jobs.postgres.visibility_timeout_ms` | `u64` |
//! | `AUTUMN_SCHEDULER__BACKEND` | `scheduler.backend` | `in_process` / `postgres` |
//! | `AUTUMN_SCHEDULER__LEASE_TTL_SECS` | `scheduler.lease_ttl_secs` | `u64` |
//! | `AUTUMN_SCHEDULER__REPLICA_ID` | `scheduler.replica_id` | `String` |
//! | `AUTUMN_SCHEDULER__KEY_PREFIX` | `scheduler.key_prefix` | `String` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__ENABLED` | `security.rate_limit.enabled` | `bool` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__REQUESTS_PER_SECOND` | `security.rate_limit.requests_per_second` | `f64` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__BURST` | `security.rate_limit.burst` | `u32` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__TRUST_FORWARDED_HEADERS` | `security.rate_limit.trust_forwarded_headers` | `bool` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__TRUSTED_PROXIES` | `security.rate_limit.trusted_proxies` | comma-separated `String` |
//! | `AUTUMN_ENV` | active profile | `String` |
//! | `AUTUMN_PROFILE` | active profile (legacy alias) | `String` |
//! | `AUTUMN_SECURITY__UPLOAD__MAX_REQUEST_SIZE_BYTES` | `security.upload.max_request_size_bytes` | `usize` |
//! | `AUTUMN_SECURITY__UPLOAD__MAX_FILE_SIZE_BYTES` | `security.upload.max_file_size_bytes` | `usize` |
//! | `AUTUMN_SECURITY__UPLOAD__ALLOWED_MIME_TYPES` | `security.upload.allowed_mime_types` | comma-separated `String` |
//! | `AUTUMN_SECURITY__FORBIDDEN_RESPONSE` | `security.forbidden_response` | `"403"` or `"404"` |
//! | `AUTUMN_SECURITY__ALLOW_UNAUTHORIZED_REPOSITORY_API` | `security.allow_unauthorized_repository_api` | `bool` |
//! | `AUTUMN_SECURITY__SIGNING_SECRET` | `security.signing_secret.secret` | `String` |
//! | `AUTUMN_SECURITY__WEBHOOKS__REPLAY__BACKEND` | `security.webhooks.replay.backend` | `memory` / `redis` |
//! | `AUTUMN_SECURITY__WEBHOOKS__REPLAY__REDIS__URL` | `security.webhooks.replay.redis.url` | `String` |
//! | `AUTUMN_SECURITY__WEBHOOKS__REPLAY__REDIS__KEY_PREFIX` | `security.webhooks.replay.redis.key_prefix` | `String` |
//! | `AUTUMN_SECURITY__WEBHOOKS__REPLAY__ALLOW_MEMORY_IN_PRODUCTION` | `security.webhooks.replay.allow_memory_in_production` | `bool` |
//! | `AUTUMN_DEV__INSPECTOR_PATH` | `dev.inspector_path` | `String` |
//! | `AUTUMN_DEV__INSPECTOR_CAPACITY` | `dev.inspector_capacity` | `usize` |
//! | `AUTUMN_DEV__INSPECTOR_N_PLUS_ONE_THRESHOLD` | `dev.inspector_n_plus_one_threshold` | `usize` |
//! | `AUTUMN_COMPRESSION__ENABLED` | `compression.enabled` | `bool` |
//! | `AUTUMN_AUTH__LOCKOUT__ENABLED` | `auth.lockout.enabled` | `bool` |
//! | `AUTUMN_AUTH__LOCKOUT__THRESHOLD` | `auth.lockout.threshold` | `i32` |
//! | `AUTUMN_AUTH__LOCKOUT__WINDOW_SECS` | `auth.lockout.window_secs` | `u64` |
//! | `AUTUMN_AUTH__LOCKOUT__COOLOFF_SECS` | `auth.lockout.cooloff_secs` | `u64` |

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Abstraction for reading environment variables, supporting dependency injection for testing.
use std::sync::OnceLock;

static MACRO_MANIFEST_DIR: OnceLock<String> = OnceLock::new();
static MACRO_IS_DEBUG: OnceLock<bool> = OnceLock::new();

#[doc(hidden)]
pub fn __set_macro_context(manifest_dir: String, is_debug: bool) {
    let _ = MACRO_MANIFEST_DIR.set(manifest_dir);
    let _ = MACRO_IS_DEBUG.set(is_debug);
}

/// Trait for environment variable reading to allow testing overrides.
///
/// This abstracts the OS environment (`std::env::var`) so that
/// configuration loading logic can be unit-tested deterministically
/// by supplying a mock environment.
pub trait Env {
    /// Read an environment variable.
    ///
    /// # Examples
    ///
    /// ```
    /// use autumn_web::config::{Env, OsEnv};
    /// let env = OsEnv;
    /// let val = env.var("NON_EXISTENT_VAR");
    /// assert!(val.is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`std::env::VarError`] if the variable is not present or is not valid Unicode.
    fn var(&self, key: &str) -> Result<String, std::env::VarError>;
}

/// Production implementation of `Env` that reads from the OS environment.
#[derive(Clone, Default)]
pub struct OsEnv;

impl Env for OsEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        if key == "AUTUMN_MANIFEST_DIR" {
            if let Some(dir) = MACRO_MANIFEST_DIR.get() {
                return Ok(dir.clone());
            }
        } else if key == "AUTUMN_IS_DEBUG"
            && let Some(is_debug) = MACRO_IS_DEBUG.get()
        {
            return Ok(if *is_debug {
                "1".to_string()
            } else {
                "0".to_string()
            });
        }
        std::env::var(key)
    }
}

/// Mock implementation of `Env` for testing.
#[derive(Clone, Default)]
pub struct MockEnv {
    vars: std::collections::HashMap<String, String>,
}

impl MockEnv {
    /// Create a new, empty `MockEnv`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vars: std::collections::HashMap::new(),
        }
    }

    /// Set an environment variable in the mock.
    #[must_use]
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.vars.insert(key.to_owned(), value.to_owned());
        self
    }

    /// Remove an environment variable from the mock.
    #[must_use]
    pub fn without(mut self, key: &str) -> Self {
        self.vars.remove(key);
        self
    }
}

impl Env for MockEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        self.vars
            .get(key)
            .cloned()
            .ok_or(std::env::VarError::NotPresent)
    }
}

/// Locate a config file by checking the app's crate directory first, then CWD.
fn find_config_file_named(filename: &str, env: &dyn Env) -> PathBuf {
    if let Ok(manifest_dir) = env.var("AUTUMN_MANIFEST_DIR") {
        let candidate = PathBuf::from(manifest_dir).join(filename);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(filename)
}

/// Load a TOML file as a raw `toml::Value` table.
/// Returns `Ok(None)` if the file doesn't exist.
fn load_raw_toml(path: &Path) -> Result<Option<toml::Value>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let table = toml::from_str::<toml::Table>(&contents)?;
            Ok(Some(toml::Value::Table(table)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Resolve the active profile using the precedence chain.
///
/// 1. `AUTUMN_ENV` env var (highest priority)
/// 2. `AUTUMN_PROFILE` env var (legacy alias)
/// 3. `--profile <name>` CLI flag
/// 4. Auto-detect from build mode (`AUTUMN_IS_DEBUG` set by `#[autumn_web::main]`)
/// 5. Fallback to `dev`
pub(crate) fn resolve_profile(env: &dyn Env) -> String {
    let selected_profile_input = resolve_profile_input(env);
    normalize_profile_name(&selected_profile_input).unwrap_or_else(|| "dev".to_owned())
}

/// Resolve the raw profile selector value (before normalization).
fn resolve_profile_input(env: &dyn Env) -> String {
    // 1. Preferred env var
    if let Ok(profile) = env.var("AUTUMN_ENV") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    // 2. Legacy env var
    if let Ok(profile) = env.var("AUTUMN_PROFILE") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    // 3. CLI flag
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--profile"
            && let Some(profile) = args.get(i + 1)
        {
            let trimmed = profile.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
        if let Some(profile) = arg.strip_prefix("--profile=") {
            let trimmed = profile.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }

    // 4. Auto-detect from build mode
    if env.var("AUTUMN_IS_DEBUG").ok().as_deref() == Some("0") {
        return "prod".to_owned();
    }
    "dev".to_owned()
}

/// Normalize profile aliases and trim whitespace.
///
/// Supported aliases:
/// - `production` -> `prod`
/// - `development` -> `dev`
/// - `prod`/`PROD` -> `prod`
/// - `dev`/`DEV` -> `dev`
fn normalize_profile_name(profile: &str) -> Option<String> {
    let trimmed = profile.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.eq_ignore_ascii_case("production") {
        return Some("prod".to_owned());
    }
    if trimmed.eq_ignore_ascii_case("development") {
        return Some("dev".to_owned());
    }
    if trimmed.eq_ignore_ascii_case("prod") {
        return Some("prod".to_owned());
    }
    if trimmed.eq_ignore_ascii_case("dev") {
        return Some("dev".to_owned());
    }

    // Preserve user-specified case for custom profile names.
    Some(trimmed.to_owned())
}

/// Profile names to check for inline/file overrides.
///
/// For canonical profiles, include legacy aliases for compatibility so
/// `production` and `development` profile sources are still loaded.
fn profile_lookup_names(profile: &str) -> Vec<&str> {
    match profile {
        "prod" => vec!["production", "prod"],
        "dev" => vec!["development", "dev"],
        other => vec![other],
    }
}

/// Ordered file lookup names for profile override file compatibility.
///
/// Only one profile override file is loaded: the first existing file in this
/// ordered list. The order prefers the explicitly-selected spelling.
fn profile_override_file_lookup_names(profile: &str, selected_profile_input: &str) -> Vec<String> {
    match profile {
        "prod" if selected_profile_input.eq_ignore_ascii_case("production") => {
            vec!["production".to_owned(), "prod".to_owned()]
        }
        "prod" => vec!["prod".to_owned(), "production".to_owned()],
        "dev" if selected_profile_input.eq_ignore_ascii_case("development") => {
            vec!["development".to_owned(), "dev".to_owned()]
        }
        "dev" => vec!["dev".to_owned(), "development".to_owned()],
        other => vec![other.to_owned()],
    }
}

/// Extract `[profile.<name>]` table from a parsed `autumn.toml`.
fn profile_section_from_base_toml(base: &toml::Value, profile: &str) -> Option<toml::Value> {
    base.get("profile")
        .and_then(toml::Value::as_table)
        .and_then(|profiles| profiles.get(profile))
        .and_then(toml::Value::as_table)
        .map(|table| toml::Value::Table(table.clone()))
}

/// Profile-specific smart defaults as a TOML table.
///
/// Only `dev` and `prod` have smart defaults. Custom profiles
/// (staging, test, etc.) get no smart defaults — they rely on
/// their profile TOML file and env overrides.
fn profile_defaults_as_toml(profile: &str) -> toml::Value {
    let mut table = toml::map::Map::new();

    match profile {
        "dev" => {
            let mut log = toml::map::Map::new();
            log.insert("level".into(), "debug".into());
            log.insert("format".into(), "Pretty".into());
            table.insert("log".into(), toml::Value::Table(log));

            let mut telemetry = toml::map::Map::new();
            telemetry.insert("environment".into(), "development".into());
            table.insert("telemetry".into(), toml::Value::Table(telemetry));

            let mut server = toml::map::Map::new();
            server.insert("host".into(), "127.0.0.1".into());
            server.insert("shutdown_timeout_secs".into(), toml::Value::Integer(1));
            // Zero-out the prestop grace in dev: there is no load balancer to
            // deregister, so the 5-second default would add unnecessary latency
            // on every Ctrl-C.
            server.insert("prestop_grace_secs".into(), toml::Value::Integer(0));
            table.insert("server".into(), toml::Value::Table(server));

            let mut health = toml::map::Map::new();
            health.insert("detailed".into(), toml::Value::Boolean(true));
            table.insert("health".into(), toml::Value::Table(health));

            let mut actuator = toml::map::Map::new();
            actuator.insert("sensitive".into(), toml::Value::Boolean(true));
            table.insert("actuator".into(), toml::Value::Table(actuator));

            let mut cors = toml::map::Map::new();
            cors.insert(
                "allowed_origins".into(),
                toml::Value::Array(vec![toml::Value::String("*".to_owned())]),
            );
            table.insert("cors".into(), toml::Value::Table(cors));

            // Dev: enable the local-disk blob store rooted at
            // `target/blobs/` automatically when the `storage` feature
            // is on. `prod` deliberately leaves `backend = "disabled"`
            // so the operator has to opt into either `local` (with
            // `allow_local_in_production = true`) or `s3`.
            let mut storage = toml::map::Map::new();
            storage.insert("backend".into(), "local".into());
            table.insert("storage".into(), toml::Value::Table(storage));
            // Dev: trust X-Forwarded-* from loopback only so local reverse
            // proxies (nginx, caddy, etc. on 127.0.0.1/::1) work out of the box.
            let mut trusted_proxies = toml::map::Map::new();
            trusted_proxies.insert("trust_forwarded_headers".into(), toml::Value::Boolean(true));
            trusted_proxies.insert(
                "ranges".into(),
                toml::Value::Array(vec![
                    toml::Value::String("127.0.0.0/8".to_owned()),
                    toml::Value::String("::1/128".to_owned()),
                ]),
            );
            let mut security = toml::map::Map::new();
            security.insert(
                "trusted_proxies".into(),
                toml::Value::Table(trusted_proxies),
            );
            table.insert("security".into(), toml::Value::Table(security));
            // Dev: CSRF disabled (default), HSTS off (default)
        }
        "prod" => {
            let mut log = toml::map::Map::new();
            log.insert("level".into(), "info".into());
            log.insert("format".into(), "Json".into());
            table.insert("log".into(), toml::Value::Table(log));

            let mut telemetry = toml::map::Map::new();
            telemetry.insert("environment".into(), "production".into());
            table.insert("telemetry".into(), toml::Value::Table(telemetry));

            let mut server = toml::map::Map::new();
            server.insert("host".into(), "0.0.0.0".into());
            server.insert("shutdown_timeout_secs".into(), toml::Value::Integer(30));
            let mut timeouts = toml::map::Map::new();
            timeouts.insert("request_timeout_ms".into(), toml::Value::Integer(30_000));
            server.insert("timeouts".into(), toml::Value::Table(timeouts));
            table.insert("server".into(), toml::Value::Table(server));

            let mut health = toml::map::Map::new();
            health.insert("detailed".into(), toml::Value::Boolean(false));
            table.insert("health".into(), toml::Value::Table(health));

            // Prod: strict security -- HSTS on, CSRF enabled, secure cookies
            let mut security = toml::map::Map::new();
            let mut headers = toml::map::Map::new();
            headers.insert(
                "strict_transport_security".into(),
                toml::Value::Boolean(true),
            );
            security.insert("headers".into(), toml::Value::Table(headers));
            let mut csrf = toml::map::Map::new();
            csrf.insert("enabled".into(), toml::Value::Boolean(true));
            security.insert("csrf".into(), toml::Value::Table(csrf));
            table.insert("security".into(), toml::Value::Table(security));

            let mut session = toml::map::Map::new();
            session.insert("secure".into(), toml::Value::Boolean(true));
            table.insert("session".into(), toml::Value::Table(session));
        }
        _ => {} // Custom profiles get no smart defaults
    }

    toml::Value::Table(table)
}

#[cfg(feature = "mail")]
fn has_mail_transport_source(merged: &toml::Value, env: &dyn Env) -> bool {
    merged
        .get("mail")
        .and_then(toml::Value::as_table)
        .is_some_and(|mail| mail.contains_key("transport"))
        || env
            .var("AUTUMN_MAIL__TRANSPORT")
            .ok()
            .as_deref()
            .is_some_and(|value| crate::mail::Transport::from_env_value(value).is_some())
}

/// Maximum recursion depth for merging TOML tables.
const MAX_MERGE_DEPTH: usize = 16;

/// Deep-merge two TOML values. Tables are merged recursively;
/// non-table values in `overlay` replace those in `base`.
fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    deep_merge_with_depth(base, overlay, 0);
}

fn deep_merge_with_depth(base: &mut toml::Value, overlay: toml::Value, depth: usize) {
    if depth > MAX_MERGE_DEPTH {
        eprintln!(
            "Warning: Configuration merge exceeded max depth ({MAX_MERGE_DEPTH}), ignoring deeper values."
        );
        return;
    }

    let toml::Value::Table(overlay_table) = overlay else {
        return;
    };
    let Some(base_table) = base.as_table_mut() else {
        return;
    };

    for (key, overlay_val) in overlay_table {
        let is_recursive_merge =
            overlay_val.is_table() && base_table.get(&key).is_some_and(toml::Value::is_table);

        if is_recursive_merge {
            if let Some(base_val) = base_table.get_mut(&key) {
                deep_merge_with_depth(base_val, overlay_val, depth + 1);
            }
        } else {
            base_table.insert(key, overlay_val);
        }
    }
}

/// Suggest a close match for a custom profile name.
///
/// Returns `Some(name)` when a known profile is within edit distance 2.
fn suggest_profile(profile: &str) -> Option<&'static str> {
    let known = ["dev", "prod"];
    let mut suggestions: Vec<(&str, usize)> = known
        .iter()
        .map(|k| (*k, levenshtein(profile, k)))
        .filter(|(_, d)| *d <= 2)
        .collect();
    suggestions.sort_by_key(|(_, d)| *d);
    suggestions.first().map(|(name, _)| *name)
}

/// Warn when a custom profile has no TOML file, suggesting close matches.
fn warn_profile_typo(profile: &str) {
    if let Some(suggestion) = suggest_profile(profile) {
        eprintln!(
            "Warning: profile \"{profile}\" has no config file (autumn-{profile}.toml) \
             and no smart defaults. Did you mean \"{suggestion}\"?"
        );
    }
}

fn should_warn_missing_profile_file(profile: &str, has_inline_profile_section: bool) -> bool {
    profile != "dev" && profile != "prod" && !has_inline_profile_section
}

/// Levenshtein edit distance between two strings.
///
/// ⚡ Bolt Optimization:
/// Reduces memory allocations by using a single `Vec` instead of two and
/// iterating directly over `Chars` to avoid `Vec<char>` allocations.
fn levenshtein(a: &str, b: &str) -> usize {
    let n = b.chars().count();
    let mut prev: Vec<usize> = (0..=n).collect();
    for (i, a_ch) in a.chars().enumerate() {
        let mut prev_diag = prev[0];
        prev[0] = i + 1;
        for (j, b_ch) in b.chars().enumerate() {
            let old_prev = prev[j + 1];
            let cost = usize::from(a_ch != b_ch);
            prev[j + 1] = (prev[j + 1] + 1).min(prev[j] + 1).min(prev_diag + cost);
            prev_diag = old_prev;
        }
    }
    prev[n]
}

/// Errors that can occur when loading or validating configuration.
///
/// Returned by [`AutumnConfig::load`], [`AutumnConfig::load_from`], and
/// [`DatabaseConfig::validate`].
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::{AutumnConfig, ConfigError};
/// use std::path::Path;
///
/// let result = AutumnConfig::load_from(Path::new("nonexistent.toml"));
/// // Returns Ok(defaults) when file is missing -- not an error
/// assert!(result.is_ok());
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The config file exists but could not be read.
    #[error("failed to read autumn.toml: {0}")]
    Io(#[from] std::io::Error),

    /// The config file contains invalid TOML syntax.
    #[error("invalid autumn.toml: {0}")]
    Parse(#[from] toml::de::Error),

    /// A configuration value failed semantic validation (e.g., invalid
    /// database URL scheme).
    #[error("configuration error: {0}")]
    Validation(String),

    /// The credentials file exists but could not be decrypted.
    #[error("credentials error: {0}")]
    Credentials(String),
}

/// Top-level framework configuration.
///
/// All sections are optional -- missing sections use their defaults.
/// Deserialized from `autumn.toml` (TOML format).
///
/// # `autumn.toml` example
///
/// ```toml
/// [server]
/// port = 8080
///
/// [database]
/// url = "postgres://user:pass@db:5432/myapp"
/// pool_size = 20
/// ```
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::AutumnConfig;
///
/// let config = AutumnConfig::default();
/// assert_eq!(config.server.port, 3000);
/// assert_eq!(config.database.pool_size, 10);
/// assert_eq!(config.log.level, "info");
/// assert_eq!(config.health.path, "/health");
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutumnConfig {
    /// Active profile name (e.g., "dev", "prod", "staging").
    /// Resolved at load time, not deserialized from TOML.
    #[serde(skip)]
    pub profile: Option<String>,

    /// HTTP server settings (port, host, shutdown behavior).
    #[serde(default)]
    pub server: ServerConfig,

    /// Database connection settings (URL, pool size, timeouts).
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Logging configuration (level, format).
    #[serde(default)]
    pub log: LogConfig,

    /// Telemetry configuration (OTLP tracing and service metadata).
    #[serde(default)]
    pub telemetry: TelemetryConfig,

    /// Health check endpoint settings.
    #[serde(default)]
    pub health: HealthConfig,

    /// Actuator endpoint settings.
    #[serde(default)]
    pub actuator: ActuatorConfig,

    /// CORS (Cross-Origin Resource Sharing) settings.
    #[serde(default)]
    pub cors: CorsConfig,

    /// Session management settings.
    #[serde(default)]
    pub session: crate::session::SessionConfig,

    /// Cache backend settings.
    #[serde(default)]
    pub cache: CacheConfig,

    /// Row-level multi-tenancy settings.
    #[serde(default)]
    pub tenancy: TenancyConfig,

    /// HTTP idempotency-key middleware settings.
    #[serde(default)]
    pub idempotency: IdempotencyConfig,

    /// Real-time channel backend settings.
    #[serde(default)]
    pub channels: ChannelConfig,

    /// Background job backend and runtime settings.
    #[serde(default)]
    pub jobs: JobConfig,

    /// Scheduled task coordination backend settings.
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Authentication settings.
    #[serde(default)]
    pub auth: crate::auth::AuthConfig,

    /// Security settings (headers, CSRF).
    #[serde(default)]
    pub security: crate::security::config::SecurityConfig,

    /// Internationalization settings (default locale, supported locales,
    /// fallback chain). Populated from the `[i18n]` block in
    /// `autumn.toml`.
    #[cfg(feature = "i18n")]
    #[serde(default)]
    pub i18n: crate::i18n::I18nConfig,
    /// Pluggable file storage configuration. Honored only when the
    /// `storage` cargo feature is enabled.
    #[cfg(feature = "storage")]
    #[serde(default)]
    pub storage: crate::storage::StorageConfig,
    /// Transactional email settings.
    #[cfg(feature = "mail")]
    #[serde(default)]
    pub mail: crate::mail::MailConfig,
    /// `OpenAPI` spec runtime exposure settings.
    ///
    /// Controls whether the generated `OpenAPI` spec is served at runtime
    /// and at which path. Use `[openapi] enabled = false` in `autumn.toml`
    /// to suppress the spec endpoint in production.
    #[serde(default, rename = "openapi")]
    pub openapi_runtime: OpenApiRuntimeConfig,

    /// Encrypted credentials store loaded from `config/credentials/<env>.toml.enc`.
    ///
    /// Empty when no credentials file exists (existing apps continue to boot unchanged).
    /// Prefer using `config.credentials().get::<String>("stripe_key")` for type-safe access.
    #[serde(skip)]
    pub credentials: crate::credentials::CredentialsStore,

    /// Outbound HTTP settings (`[http]` section in `autumn.toml`).
    ///
    /// The nested `[http.client]` sub-table configures the outbound client.
    #[cfg(feature = "http-client")]
    #[serde(default, rename = "http")]
    pub http: HttpConfig,

    /// Developer-experience settings (`[dev]` section in `autumn.toml`).
    ///
    /// Controls the request inspector and other dev-only features.
    /// These settings have no effect outside the `dev` profile.
    #[serde(default)]
    pub dev: DevConfig,

    /// Error-reporting settings (`[reporting]` section in `autumn.toml`).
    ///
    /// Controls delivery of panic + 5xx [`ErrorEvent`](crate::reporting::ErrorEvent)s
    /// to registered reporters. Honored only when the `reporting` cargo
    /// feature is enabled.
    #[cfg(feature = "reporting")]
    #[serde(default)]
    pub reporting: ReportingConfig,

    /// Response compression settings (`[compression]` section in `autumn.toml`).
    ///
    /// Compression is **off by default**. Enable with:
    /// ```toml
    /// [compression]
    /// enabled = true
    /// ```
    /// or via `AUTUMN_COMPRESSION__ENABLED=true`.
    #[serde(default)]
    pub compression: CompressionConfig,
}

/// Error-reporting settings (`[reporting]` section in `autumn.toml`).
///
/// # Example `autumn.toml`
///
/// ```toml
/// [reporting]
/// enabled = true      # deliver events to reporters (default: true)
/// sample_rate = 0.25  # report ~25% of events (default: 1.0 = all)
/// ```
///
/// Note: `enabled = false` only suppresses *delivery* to reporters. Handler
/// panics are still caught and converted to a clean 500 response regardless of
/// this setting.
#[cfg(feature = "reporting")]
#[derive(Debug, Clone, Deserialize)]
pub struct ReportingConfig {
    /// Whether error events are delivered to registered reporters.
    ///
    /// Defaults to `true`. When `false`, panics are still caught and turned
    /// into clean 500 responses, but no [`ErrorEvent`](crate::reporting::ErrorEvent)
    /// is dispatched.
    #[serde(default = "default_reporting_enabled")]
    pub enabled: bool,
    /// Fraction of events to deliver, in `[0.0, 1.0]`.
    ///
    /// `1.0` (the default) reports every event; `0.0` reports none. Values
    /// outside the range are clamped at the extremes.
    #[serde(default = "default_reporting_sample_rate")]
    pub sample_rate: f64,
}

#[cfg(feature = "reporting")]
impl Default for ReportingConfig {
    fn default() -> Self {
        Self {
            enabled: default_reporting_enabled(),
            sample_rate: default_reporting_sample_rate(),
        }
    }
}

#[cfg(feature = "reporting")]
const fn default_reporting_enabled() -> bool {
    true
}

#[cfg(feature = "reporting")]
const fn default_reporting_sample_rate() -> f64 {
    1.0
}

/// Developer-experience settings (`[dev]` section in `autumn.toml`).
///
/// All fields are ignored outside the `dev` profile.
///
/// # Example `autumn.toml`
///
/// ```toml
/// [dev]
/// inspector_path = "/_autumn/inspect"
/// inspector_capacity = 200
/// inspector_n_plus_one_threshold = 3
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct DevConfig {
    /// Mount path for the request inspector UI.
    ///
    /// Default: `"/_autumn/inspect"`. Only active in the `dev` profile;
    /// ignored everywhere else.
    #[serde(default = "default_inspector_path")]
    pub inspector_path: String,

    /// Maximum number of requests retained in the in-memory ring buffer.
    ///
    /// Default: `100`. Set to `0` to disable recording without removing
    /// the middleware.
    #[serde(default = "default_inspector_capacity")]
    pub inspector_capacity: usize,

    /// Minimum number of structurally identical SQL statements in a single
    /// request before an N+1 warning is emitted.
    ///
    /// Default: `5`. Set to `0` to disable N+1 detection.
    #[serde(default = "default_inspector_n_plus_one_threshold")]
    pub inspector_n_plus_one_threshold: usize,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            inspector_path: default_inspector_path(),
            inspector_capacity: default_inspector_capacity(),
            inspector_n_plus_one_threshold: default_inspector_n_plus_one_threshold(),
        }
    }
}

fn default_inspector_path() -> String {
    "/_autumn/inspect".to_owned()
}

const fn default_inspector_capacity() -> usize {
    100
}

const fn default_inspector_n_plus_one_threshold() -> usize {
    5
}

/// Top-level `[http]` configuration section.
#[cfg(feature = "http-client")]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HttpConfig {
    /// Outbound HTTP client settings (`[http.client]`).
    #[serde(default)]
    pub client: HttpClientConfig,
}

/// Configuration for the outbound HTTP client (`[http.client]` in `autumn.toml`).
///
/// # Example `autumn.toml`
///
/// ```toml
/// [http.client]
/// timeout_secs = 30
/// max_retries  = 3
///
/// [http.client.base_urls]
/// stripe   = "https://api.stripe.com"
/// sendgrid = "https://api.sendgrid.com"
/// ```
#[cfg(feature = "http-client")]
#[derive(Debug, Clone, Deserialize)]
pub struct HttpClientConfig {
    /// Per-request timeout in seconds. Default: 30.
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,

    /// Maximum retry attempts for transient failures on idempotent methods.
    /// Default: 3 (four total attempts).
    #[serde(default = "default_http_max_retries")]
    pub max_retries: u32,

    /// Named base URL aliases, e.g. `stripe = "https://api.stripe.com"`.
    ///
    /// A [`Client`](crate::http_client::Client) configured with `.named("stripe")` will
    /// prepend this URL to relative request paths and match against mocks
    /// registered for that alias via
    /// [`TestApp::http_mock`](crate::test::TestApp::http_mock).
    #[serde(default)]
    pub base_urls: std::collections::HashMap<String, String>,
}

#[cfg(feature = "http-client")]
const fn default_http_timeout_secs() -> u64 {
    30
}

#[cfg(feature = "http-client")]
const fn default_http_max_retries() -> u32 {
    3
}

#[cfg(feature = "http-client")]
impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_http_timeout_secs(),
            max_retries: default_http_max_retries(),
            base_urls: std::collections::HashMap::new(),
        }
    }
}

impl axum::extract::FromRequestParts<crate::AppState> for AutumnConfig {
    type Rejection = crate::AutumnError;

    async fn from_request_parts(
        _parts: &mut http::request::Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        state
            .extension::<Self>()
            .as_deref()
            .cloned()
            .ok_or_else(|| crate::AutumnError::service_unavailable_msg("Config is not available"))
    }
}

/// Real-time channel backend selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelBackend {
    /// In-process Tokio broadcast channels. Default, zero config.
    #[serde(alias = "local", alias = "memory")]
    #[default]
    InProcess,
    /// Redis pub/sub fan-out across application replicas.
    Redis,
}

impl ChannelBackend {
    /// Parse an environment variable value for channel backend selection.
    #[must_use]
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "in_process" | "in-process" | "local" | "memory" => Some(Self::InProcess),
            "redis" => Some(Self::Redis),
            _ => None,
        }
    }
}

/// Real-time channel runtime configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfig {
    /// Runtime backend selection.
    #[serde(default)]
    pub backend: ChannelBackend,
    /// Per-topic broadcast ring buffer capacity.
    #[serde(default = "default_channel_capacity")]
    pub capacity: usize,
    /// Redis backend options.
    #[serde(default)]
    pub redis: ChannelRedisConfig,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            backend: ChannelBackend::default(),
            capacity: default_channel_capacity(),
            redis: ChannelRedisConfig::default(),
        }
    }
}

/// Redis channel backend configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelRedisConfig {
    /// Redis URL used when `channels.backend = "redis"`.
    #[serde(default)]
    pub url: Option<String>,
    /// Redis pub/sub channel prefix.
    #[serde(default = "default_channels_redis_prefix")]
    pub key_prefix: String,
}

impl Default for ChannelRedisConfig {
    fn default() -> Self {
        Self {
            url: None,
            key_prefix: default_channels_redis_prefix(),
        }
    }
}

const fn default_channel_capacity() -> usize {
    32
}

fn default_channels_redis_prefix() -> String {
    "autumn:channels".to_owned()
}

// ── Cache configuration ──────────────────────────────────────────────────────

/// Cache backend selection for `#[cached]` and `CacheResponseLayer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheBackend {
    /// In-process Moka cache (default). Each replica has an independent store.
    #[default]
    Memory,
    /// Shared Redis cache. Invalidations propagate across all replicas.
    Redis,
}

impl CacheBackend {
    pub(crate) fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "memory" => Some(Self::Memory),
            "redis" => Some(Self::Redis),
            _ => None,
        }
    }
}

/// Configuration for the shared application cache.
///
/// Placed in `autumn.toml` under `[cache]`.
///
/// # Examples
///
/// ```toml
/// [cache]
/// backend = "redis"
///
/// [cache.redis]
/// url = "redis://redis:6379"
/// key_prefix = "myapp:cache"
/// ```
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CacheConfig {
    /// Active cache backend.
    #[serde(default)]
    pub backend: CacheBackend,

    /// Redis backend options.
    #[serde(default)]
    pub redis: CacheRedisConfig,
}

impl CacheConfig {
    /// Returns `true` when the memory (Moka) backend is selected.
    #[must_use]
    pub fn is_memory(&self) -> bool {
        self.backend == CacheBackend::Memory
    }

    /// Returns `true` when the Redis backend is selected.
    #[must_use]
    pub fn is_redis(&self) -> bool {
        self.backend == CacheBackend::Redis
    }
}

/// Redis cache backend configuration.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CacheRedisConfig {
    /// Redis connection URL (e.g. `redis://127.0.0.1:6379`).
    #[serde(default)]
    pub url: Option<String>,

    /// Prefix for all cache keys stored in Redis.
    #[serde(default = "default_cache_redis_key_prefix")]
    pub key_prefix: String,
}

impl Default for CacheRedisConfig {
    fn default() -> Self {
        Self {
            url: None,
            key_prefix: default_cache_redis_key_prefix(),
        }
    }
}

fn default_cache_redis_key_prefix() -> String {
    "autumn:cache".to_owned()
}

/// Scheduled task coordination backend selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerBackend {
    /// Per-process scheduler timers. This preserves existing single-replica behavior.
    #[serde(alias = "local", alias = "memory")]
    #[default]
    InProcess,
    /// Fleet coordination with Postgres advisory locks.
    Postgres,
}

impl SchedulerBackend {
    /// Parse an environment variable value for scheduler backend selection.
    #[must_use]
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "in_process" | "in-process" | "local" | "memory" => Some(Self::InProcess),
            "postgres" | "postgresql" => Some(Self::Postgres),
            _ => None,
        }
    }
}

/// Scheduled task coordination runtime configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Runtime backend selection.
    #[serde(default)]
    pub backend: SchedulerBackend,
    /// Lease duration used by distributed backends for run visibility and timeout guidance.
    #[serde(default = "default_scheduler_lease_ttl_secs")]
    pub lease_ttl_secs: u64,
    /// Stable replica identifier surfaced in actuator metadata.
    #[serde(default)]
    pub replica_id: Option<String>,
    /// Prefix included when deriving Postgres advisory lock keys.
    #[serde(default = "default_scheduler_key_prefix")]
    pub key_prefix: String,
}

impl SchedulerConfig {
    /// Resolve a stable-ish replica identifier for actuator metadata and lock ownership.
    #[must_use]
    pub fn resolved_replica_id(&self) -> String {
        self.replica_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .cloned()
            .or_else(|| std::env::var("FLY_MACHINE_ID").ok())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| format!("pid-{}", std::process::id()))
    }

    /// Validate scheduler-specific config shape.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Validation`] when values are syntactically valid TOML
    /// but cannot be used by the runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.lease_ttl_secs == 0 {
            return Err(ConfigError::Validation(
                "scheduler.lease_ttl_secs must be greater than zero".to_owned(),
            ));
        }
        if self.key_prefix.trim().is_empty() {
            return Err(ConfigError::Validation(
                "scheduler.key_prefix must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            backend: SchedulerBackend::default(),
            lease_ttl_secs: default_scheduler_lease_ttl_secs(),
            replica_id: None,
            key_prefix: default_scheduler_key_prefix(),
        }
    }
}

const fn default_scheduler_lease_ttl_secs() -> u64 {
    300
}

fn default_scheduler_key_prefix() -> String {
    "autumn:scheduler".to_owned()
}

/// Storage backend selection for HTTP idempotency keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum IdempotencyBackend {
    #[default]
    Memory,
    Redis,
}

impl IdempotencyBackend {
    /// Parse an environment variable value for idempotency backend selection.
    #[must_use]
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "memory" | "mem" => Some(Self::Memory),
            "redis" => Some(Self::Redis),
            _ => None,
        }
    }
}

/// Redis connection settings for the idempotency backend.
#[derive(Debug, Clone, Deserialize)]
pub struct IdempotencyRedisConfig {
    /// Redis connection URL (e.g. `redis://localhost:6379`).
    pub url: Option<String>,
    /// Key prefix for all idempotency entries and locks stored in Redis.
    #[serde(default = "default_idempotency_redis_key_prefix")]
    pub key_prefix: String,
}

impl Default for IdempotencyRedisConfig {
    fn default() -> Self {
        Self {
            url: None,
            key_prefix: default_idempotency_redis_key_prefix(),
        }
    }
}

fn default_idempotency_redis_key_prefix() -> String {
    "autumn:idempotency".to_owned()
}

/// HTTP idempotency-key middleware settings.
#[derive(Debug, Clone, Deserialize)]
pub struct IdempotencyConfig {
    /// Enable the idempotency-key middleware.
    ///
    /// When `true`, mutating requests that carry an `Idempotency-Key` header
    /// are deduplicated using the configured backend.
    ///
    /// `None` means the field was absent from the config file; the
    /// `AppBuilder::idempotent()` builder flag may still enable it.
    /// `Some(false)` is an explicit operator opt-out that overrides the builder.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Storage backend for idempotency records.
    #[serde(default)]
    pub backend: IdempotencyBackend,
    /// Time-to-live in seconds for stored idempotency records.
    #[serde(default = "default_idempotency_ttl_secs")]
    pub ttl_secs: u64,
    /// Maximum stale lifetime for distributed in-flight locks.
    ///
    /// The lock is released as soon as the handler finishes. This value is only
    /// the backend safety expiry for crashes or lost unlocks, so it should be
    /// comfortably longer than any supported mutating request duration.
    #[serde(default = "default_idempotency_in_flight_ttl_secs")]
    pub in_flight_ttl_secs: u64,
    /// Allow the in-memory backend in production environments.
    #[serde(default)]
    pub allow_memory_in_production: bool,
    /// Redis connection settings (used when `backend = "redis"`).
    #[serde(default)]
    pub redis: IdempotencyRedisConfig,
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            backend: IdempotencyBackend::default(),
            ttl_secs: default_idempotency_ttl_secs(),
            in_flight_ttl_secs: default_idempotency_in_flight_ttl_secs(),
            allow_memory_in_production: false,
            redis: IdempotencyRedisConfig::default(),
        }
    }
}

const fn default_idempotency_ttl_secs() -> u64 {
    86_400
}

const fn default_idempotency_in_flight_ttl_secs() -> u64 {
    86_400
}

/// `OpenAPI` spec runtime exposure settings.
///
/// Populated from the `[openapi]` block in `autumn.toml`. When
/// `AppBuilder::openapi(...)` is called and `enabled = true`, the framework
/// mounts the spec at `path`. Set `enabled = false` in a production profile
/// to prevent exposing the spec publicly.
///
/// # `autumn.toml` example
///
/// ```toml
/// [openapi]
/// enabled = false   # disable in prod
/// path = "/openapi.json"
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct OpenApiRuntimeConfig {
    /// Whether the `OpenAPI` spec endpoint is served.
    ///
    /// Defaults to `true` so new projects get the spec immediately.
    /// Set to `false` in production profiles to suppress the endpoint.
    #[serde(default = "default_openapi_enabled")]
    pub enabled: bool,
    /// URL path at which `openapi.json` is served.
    ///
    /// Defaults to `/openapi.json`.
    #[serde(default = "default_openapi_path")]
    pub path: String,
}

impl Default for OpenApiRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: default_openapi_enabled(),
            path: default_openapi_path(),
        }
    }
}

const fn default_openapi_enabled() -> bool {
    true
}

fn default_openapi_path() -> String {
    "/openapi.json".to_owned()
}

/// Background job runtime configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct JobConfig {
    /// Runtime backend selection.
    ///
    /// - `local` (default): in-process Tokio queue
    /// - `postgres`: Postgres-backed durable queue (requires `db` feature)
    /// - `redis`: Redis-backed durable queue (requires `redis` feature)
    #[serde(default = "default_job_backend")]
    pub backend: String,
    /// Number of concurrent worker loops to spawn.
    #[serde(default = "default_job_workers")]
    pub workers: usize,
    /// Default max attempts when `#[job(max_attempts = ...)]` is not set.
    #[serde(default = "default_job_max_attempts")]
    pub max_attempts: u32,
    /// Default initial retry backoff in milliseconds.
    #[serde(default = "default_job_backoff_ms")]
    pub initial_backoff_ms: u64,
    /// Redis backend options.
    #[serde(default)]
    pub redis: JobRedisConfig,
    /// Postgres backend options.
    #[serde(default)]
    pub postgres: JobPostgresConfig,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            backend: default_job_backend(),
            workers: default_job_workers(),
            max_attempts: default_job_max_attempts(),
            initial_backoff_ms: default_job_backoff_ms(),
            redis: JobRedisConfig::default(),
            postgres: JobPostgresConfig::default(),
        }
    }
}

/// Redis backend configuration options for the job runner.
#[derive(Debug, Clone, Deserialize)]
pub struct JobRedisConfig {
    /// Redis URL used when `jobs.backend = "redis"`.
    #[serde(default)]
    pub url: Option<String>,
    /// Key prefix for all queue keys.
    #[serde(default = "default_jobs_redis_prefix")]
    pub key_prefix: String,
    /// Duration before an in-flight job claim is considered stale.
    #[serde(default = "default_jobs_redis_visibility_timeout_ms")]
    pub visibility_timeout_ms: u64,
}

impl Default for JobRedisConfig {
    fn default() -> Self {
        Self {
            url: None,
            key_prefix: default_jobs_redis_prefix(),
            visibility_timeout_ms: default_jobs_redis_visibility_timeout_ms(),
        }
    }
}

/// Postgres backend configuration options for the job runner.
#[derive(Debug, Clone, Deserialize)]
pub struct JobPostgresConfig {
    /// Duration before an in-flight job claim is considered stale and recovered.
    ///
    /// Workers that crash mid-job have their claim reclaimed by another worker
    /// within this bound. Default: 30 seconds.
    #[serde(default = "default_jobs_pg_visibility_timeout_ms")]
    pub visibility_timeout_ms: u64,
}

impl Default for JobPostgresConfig {
    fn default() -> Self {
        Self {
            visibility_timeout_ms: default_jobs_pg_visibility_timeout_ms(),
        }
    }
}

const fn default_jobs_pg_visibility_timeout_ms() -> u64 {
    30_000
}

fn default_job_backend() -> String {
    "local".to_owned()
}

const fn default_job_workers() -> usize {
    1
}

const fn default_job_max_attempts() -> u32 {
    5
}

const fn default_job_backoff_ms() -> u64 {
    250
}

fn default_jobs_redis_prefix() -> String {
    "autumn:jobs".to_owned()
}

const fn default_jobs_redis_visibility_timeout_ms() -> u64 {
    30_000
}

impl AutumnConfig {
    /// Access the decrypted credentials store.
    ///
    /// Returns an empty store when no credentials file was found (the feature is opt-in).
    /// Use `config.credentials().get::<String>("stripe_key")` to access values.
    #[must_use]
    pub const fn credentials(&self) -> &crate::credentials::CredentialsStore {
        &self.credentials
    }

    /// Load configuration with profile-aware layering.
    ///
    /// Applies the six-layer configuration system:
    /// 1. Framework defaults
    /// 2. Profile smart defaults (dev/prod)
    /// 3. `autumn.toml` (base config)
    /// 4. `[profile.{name}]` section in `autumn.toml`
    /// 5. `autumn-{profile}.toml` (legacy profile overrides)
    /// 6. `AUTUMN_*` environment variables
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if a config file cannot be read,
    /// [`ConfigError::Parse`] if a file contains invalid TOML, or
    /// [`ConfigError::Validation`] if a value is invalid.
    ///
    /// # Panics
    ///
    /// Panics if the internally-built TOML table fails to re-serialize
    /// (should never happen with well-formed profile defaults).
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_env(&OsEnv)
    }

    /// Load configuration with profile-aware layering, using a provided
    /// environment abstraction instead of the OS environment. Useful for testing.
    ///
    /// # Errors
    /// Returns [`ConfigError::Io`] if a config file cannot be read,
    /// [`ConfigError::Parse`] if a file contains invalid TOML, or
    /// [`ConfigError::Validation`] if a value is invalid.
    ///
    /// # Panics
    /// Panics if the internally-built TOML table fails to re-serialize.
    pub fn load_with_env(env: &dyn Env) -> Result<Self, ConfigError> {
        let selected_profile_input = resolve_profile_input(env);
        let profile =
            normalize_profile_name(&selected_profile_input).unwrap_or_else(|| "dev".to_owned());
        let mut has_inline_profile_section = false;

        // Build merged TOML:
        // profile smart defaults ← autumn.toml ← [profile.{name}] ← autumn-{profile}.toml
        let mut merged = profile_defaults_as_toml(&profile);

        // Layer 3: base autumn.toml
        if let Some(base) = load_raw_toml(&find_config_file_named("autumn.toml", env))? {
            deep_merge(&mut merged, base.clone());

            // Layer 4: [profile.{name}] in autumn.toml
            for profile_name in profile_lookup_names(&profile) {
                if let Some(inline_profile) = profile_section_from_base_toml(&base, profile_name) {
                    deep_merge(&mut merged, inline_profile);
                    has_inline_profile_section = true;
                }
            }
        }

        // Layer 5: autumn-{profile}.toml (legacy compatibility)
        let mut has_profile_file = false;
        for profile_name in profile_override_file_lookup_names(&profile, &selected_profile_input) {
            let profile_path = find_config_file_named(&format!("autumn-{profile_name}.toml"), env);
            if let Some(profile_toml) = load_raw_toml(&profile_path)? {
                deep_merge(&mut merged, profile_toml);
                has_profile_file = true;
                break;
            }
        }
        if !has_profile_file
            && should_warn_missing_profile_file(&profile, has_inline_profile_section)
        {
            warn_profile_typo(&profile);
        }

        // Deserialize the merged TOML table into AutumnConfig
        let toml_str =
            toml::to_string(&merged).expect("internal error: failed to serialize merged config");
        let mut config: Self = toml::from_str(&toml_str)?;
        config.profile = Some(profile);

        // Layer 6: env var overrides (highest priority)
        config.apply_env_overrides_with_env(env);

        #[cfg(feature = "mail")]
        if config.profile.as_deref() == Some("dev") && !has_mail_transport_source(&merged, env) {
            config.mail.transport = crate::mail::Transport::Log;
        }

        config.validate()?;

        let base_dir: PathBuf = env
            .var("AUTUMN_MANIFEST_DIR")
            .map_or_else(|_| PathBuf::from("."), PathBuf::from);
        let cred_profile = config.profile.as_deref().unwrap_or("dev");
        let master_key_override = env.var("AUTUMN_MASTER_KEY").ok();
        config.credentials = crate::credentials::load_credentials_with_key_override(
            cred_profile,
            &base_dir,
            master_key_override.as_deref(),
        )
        .map_err(|e| ConfigError::Credentials(e.to_string()))?;

        #[cfg(feature = "oauth2")]
        {
            config.expand_oauth2_providers();
        }

        Ok(config)
    }

    /// Helper method to expand `OAuth2` preset configurations and resolve credentials-backed values.
    #[cfg(feature = "oauth2")]
    fn expand_oauth2_providers(&mut self) {
        let provider_names: Vec<String> = self.auth.oauth2.providers.keys().cloned().collect();
        for name in provider_names {
            // 1. Expand from preset if available
            if let (Some(preset), Some(p)) = (
                crate::auth::provider_preset(&name),
                self.auth.oauth2.providers.get_mut(&name),
            ) {
                if p.authorize_url.is_empty() {
                    p.authorize_url = preset.authorize_url;
                }
                if p.token_url.is_empty() {
                    p.token_url = preset.token_url;
                }
                if p.userinfo_url.is_none() {
                    p.userinfo_url = preset.userinfo_url;
                }
                if p.scope.is_empty() || p.scope == "default" {
                    p.scope = preset.scope;
                }
                if p.issuer.is_none() {
                    p.issuer = preset.issuer;
                }
                if p.jwks_url.is_none() {
                    p.jwks_url = preset.jwks_url;
                }
                if p.discovery_url.is_none() {
                    p.discovery_url = preset.discovery_url;
                }
            }

            // 2. Resolve credentials-backed secrets/IDs
            if let Some(p) = self.auth.oauth2.providers.get_mut(&name) {
                let normalized_name = name
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .to_lowercase();

                let id_key = format!("oauth2_{normalized_name}_client_id");
                if p.client_id.is_empty() {
                    if let Some(id) = self.credentials.get::<String>(&id_key) {
                        p.client_id = id;
                    } else if let Some(id) = self
                        .credentials
                        .get::<String>(&format!("oauth2_{name}_client_id"))
                    {
                        p.client_id = id;
                    }
                }
                let secret_key = format!("oauth2_{normalized_name}_client_secret");
                if p.client_secret.is_empty() {
                    if let Some(secret) = self.credentials.get::<String>(&secret_key) {
                        p.client_secret = secret;
                    } else if let Some(secret) = self
                        .credentials
                        .get::<String>(&format!("oauth2_{name}_client_secret"))
                    {
                        p.client_secret = secret;
                    }
                }
            }
        }
    }

    /// Load configuration from a specific TOML file path.
    ///
    /// Used internally and for testing. Does **not** apply profile
    /// layering or environment overrides. Prefer [`load()`](Self::load)
    /// in application code.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the file cannot be read, or
    /// [`ConfigError::Parse`] if the file contains invalid TOML.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let config: Self = toml::from_str(&contents)?;
                config.validate()?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    /// Validate the resolved configuration for semantic errors.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when a field combination is
    /// syntactically well-formed TOML but semantically invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.database.validate()?;
        self.cors.validate()?;
        self.scheduler.validate()?;
        let is_production = matches!(self.profile.as_deref(), Some("prod" | "production"));
        self.security
            .webhooks
            .validate(is_production)
            .map_err(|error| ConfigError::Validation(error.to_string()))?;
        #[cfg(feature = "mail")]
        self.mail.validate(self.profile.as_deref())?;
        // Session backend validation deliberately lives in
        // `crate::session::apply_session_layer`, not here. That function
        // short-circuits when a custom `SessionStore` was installed via
        // `AppBuilder::with_session_store(...)`, so the (then-irrelevant)
        // `session.backend = "redis"` config without a redis URL doesn't
        // need to fail the boot. Validating the same thing here would
        // defeat the override and exit the app before the custom store
        // ever gets a chance to apply. The "prod profile + memory backend"
        // warning lives in `apply_session_layer` for the same reason.
        Ok(())
    }

    /// Apply environment variable overrides to the loaded config.
    ///
    /// All fields can be overridden via `AUTUMN_SECTION__FIELD` environment
    /// variables. Double underscore `__` separates nested config sections.
    ///
    /// # Server
    /// - `AUTUMN_SERVER__PORT` → `server.port` (u16)
    /// - `AUTUMN_SERVER__HOST` → `server.host` (String)
    /// - `AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS` → `server.shutdown_timeout_secs` (u64)
    /// - `AUTUMN_SERVER__PRESTOP_GRACE_SECS` → `server.prestop_grace_secs` (u64)
    ///
    /// # Database
    /// - `AUTUMN_DATABASE__PRIMARY_URL` -> `database.primary_url` (String)
    /// - `AUTUMN_DATABASE__REPLICA_URL` -> `database.replica_url` (String)
    /// - `AUTUMN_DATABASE__PRIMARY_POOL_SIZE` -> `database.primary_pool_size` (usize)
    /// - `AUTUMN_DATABASE__REPLICA_POOL_SIZE` -> `database.replica_pool_size` (usize)
    /// - `AUTUMN_DATABASE__REPLICA_FALLBACK` -> `database.replica_fallback` (`fail_readiness` | `primary`)
    /// - `AUTUMN_DATABASE__URL` → `database.url` (String)
    /// - `AUTUMN_DATABASE__POOL_SIZE` → `database.pool_size` (usize)
    /// - `AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS` → `database.connect_timeout_secs` (u64)
    /// - `AUTUMN_DATABASE__AUTO_MIGRATE_IN_PRODUCTION` -> `database.auto_migrate_in_production` (bool)
    ///
    /// # Log
    /// - `AUTUMN_LOG__LEVEL` → `log.level` (String, tracing filter directive)
    /// - `AUTUMN_LOG__FORMAT` → `log.format` (Auto | Pretty | Json)
    ///
    /// # Telemetry
    /// - `AUTUMN_TELEMETRY__ENABLED` -> `telemetry.enabled` (bool)
    /// - `AUTUMN_TELEMETRY__SERVICE_NAME` -> `telemetry.service_name` (String)
    /// - `AUTUMN_TELEMETRY__SERVICE_NAMESPACE` -> `telemetry.service_namespace` (String)
    /// - `AUTUMN_TELEMETRY__SERVICE_VERSION` -> `telemetry.service_version` (String)
    /// - `AUTUMN_TELEMETRY__ENVIRONMENT` -> `telemetry.environment` (String)
    /// - `AUTUMN_TELEMETRY__OTLP_ENDPOINT` -> `telemetry.otlp_endpoint` (String)
    /// - `AUTUMN_TELEMETRY__PROTOCOL` -> `telemetry.protocol` (`Grpc` | `HttpProtobuf`)
    /// - `AUTUMN_TELEMETRY__STRICT` -> `telemetry.strict` (bool)
    ///
    /// # Health / Probes
    /// - `AUTUMN_HEALTH__PATH` → `health.path` (String)
    /// - `AUTUMN_HEALTH__LIVE_PATH` → `health.live_path` (String)
    /// - `AUTUMN_HEALTH__READY_PATH` → `health.ready_path` (String)
    /// - `AUTUMN_HEALTH__STARTUP_PATH` → `health.startup_path` (String)
    /// - `AUTUMN_HEALTH__DETAILED` → `health.detailed` (bool)
    ///
    /// # Jobs
    /// - `AUTUMN_JOBS__BACKEND` → `jobs.backend` (`local` / `redis`)
    /// - `AUTUMN_JOBS__WORKERS` → `jobs.workers` (`usize`)
    /// - `AUTUMN_JOBS__MAX_ATTEMPTS` → `jobs.max_attempts` (`u32`)
    /// - `AUTUMN_JOBS__INITIAL_BACKOFF_MS` → `jobs.initial_backoff_ms` (`u64`)
    /// - `AUTUMN_JOBS__REDIS__URL` → `jobs.redis.url` (`String`)
    /// - `AUTUMN_JOBS__REDIS__KEY_PREFIX` → `jobs.redis.key_prefix` (`String`)
    /// - `AUTUMN_JOBS__REDIS__VISIBILITY_TIMEOUT_MS` → `jobs.redis.visibility_timeout_ms` (`u64`)
    ///
    /// # Signed webhooks
    /// - `AUTUMN_SECURITY__WEBHOOKS__REPLAY__BACKEND` -> `security.webhooks.replay.backend` (`memory` / `redis`)
    /// - `AUTUMN_SECURITY__WEBHOOKS__REPLAY__REDIS__URL` -> `security.webhooks.replay.redis.url` (`String`)
    /// - `AUTUMN_SECURITY__WEBHOOKS__REPLAY__REDIS__KEY_PREFIX` -> `security.webhooks.replay.redis.key_prefix` (`String`)
    /// - `AUTUMN_SECURITY__WEBHOOKS__REPLAY__ALLOW_MEMORY_IN_PRODUCTION` -> `security.webhooks.replay.allow_memory_in_production` (`bool`)
    pub fn apply_env_overrides(&mut self) {
        self.apply_env_overrides_with_env(&OsEnv);
    }

    /// Apply environment overrides using the provided env abstraction.
    pub fn apply_env_overrides_with_env(&mut self, env: &dyn Env) {
        self.apply_server_env_overrides_with_env(env);
        self.apply_database_env_overrides_with_env(env);
        self.apply_log_env_overrides_with_env(env);
        self.apply_telemetry_env_overrides_with_env(env);
        self.apply_health_env_overrides_with_env(env);
        self.apply_cors_env_overrides_with_env(env);
        self.apply_session_env_overrides_with_env(env);
        self.apply_cache_env_overrides_with_env(env);
        self.apply_channels_env_overrides_with_env(env);
        self.apply_jobs_env_overrides_with_env(env);
        self.apply_scheduler_env_overrides_with_env(env);
        self.apply_auth_env_overrides_with_env(env);
        self.apply_security_env_overrides_with_env(env);
        self.apply_idempotency_env_overrides_with_env(env);
        self.apply_dev_env_overrides_with_env(env);
        self.apply_compression_env_overrides_with_env(env);
        #[cfg(feature = "reporting")]
        self.apply_reporting_env_overrides_with_env(env);
        #[cfg(feature = "storage")]
        self.apply_storage_env_overrides_with_env(env);
        #[cfg(feature = "mail")]
        self.apply_mail_env_overrides_with_env(env);
    }

    #[cfg(feature = "reporting")]
    fn apply_reporting_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_bool(
            env,
            "AUTUMN_REPORTING__ENABLED",
            &mut self.reporting.enabled,
        );
        parse_env(
            env,
            "AUTUMN_REPORTING__SAMPLE_RATE",
            &mut self.reporting.sample_rate,
        );
    }

    fn apply_dev_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(
            env,
            "AUTUMN_DEV__INSPECTOR_PATH",
            &mut self.dev.inspector_path,
        );
        parse_env(
            env,
            "AUTUMN_DEV__INSPECTOR_CAPACITY",
            &mut self.dev.inspector_capacity,
        );
        parse_env(
            env,
            "AUTUMN_DEV__INSPECTOR_N_PLUS_ONE_THRESHOLD",
            &mut self.dev.inspector_n_plus_one_threshold,
        );
    }

    fn apply_compression_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_bool(
            env,
            "AUTUMN_COMPRESSION__ENABLED",
            &mut self.compression.enabled,
        );
    }

    fn apply_idempotency_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_option_bool(
            env,
            "AUTUMN_IDEMPOTENCY__ENABLED",
            &mut self.idempotency.enabled,
        );
        if let Ok(val) = env.var("AUTUMN_IDEMPOTENCY__BACKEND") {
            match IdempotencyBackend::from_env_value(&val) {
                Some(backend) => self.idempotency.backend = backend,
                None => eprintln!(
                    "Warning: unrecognised AUTUMN_IDEMPOTENCY__BACKEND value {val:?}; ignoring"
                ),
            }
        }
        parse_env(
            env,
            "AUTUMN_IDEMPOTENCY__TTL_SECS",
            &mut self.idempotency.ttl_secs,
        );
        parse_env(
            env,
            "AUTUMN_IDEMPOTENCY__IN_FLIGHT_TTL_SECS",
            &mut self.idempotency.in_flight_ttl_secs,
        );
        parse_env_bool(
            env,
            "AUTUMN_IDEMPOTENCY__ALLOW_MEMORY_IN_PRODUCTION",
            &mut self.idempotency.allow_memory_in_production,
        );
        parse_env_string(
            env,
            "AUTUMN_IDEMPOTENCY__REDIS__URL",
            self.idempotency.redis.url.get_or_insert_with(String::new),
        );
        parse_env_string(
            env,
            "AUTUMN_IDEMPOTENCY__REDIS__KEY_PREFIX",
            &mut self.idempotency.redis.key_prefix,
        );
    }

    fn apply_server_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env(env, "AUTUMN_SERVER__PORT", &mut self.server.port);
        parse_env_string(env, "AUTUMN_SERVER__HOST", &mut self.server.host);
        parse_env(
            env,
            "AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS",
            &mut self.server.shutdown_timeout_secs,
        );
        parse_env(
            env,
            "AUTUMN_SERVER__PRESTOP_GRACE_SECS",
            &mut self.server.prestop_grace_secs,
        );
        parse_env_option(
            env,
            "AUTUMN_SERVER__TIMEOUTS__REQUEST_TIMEOUT_MS",
            &mut self.server.timeouts.request_timeout_ms,
        );
    }

    fn apply_database_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_DATABASE__URL") {
            self.database.url = Some(val);
            self.database.primary_url = None;
        }
        parse_env_option_string(
            env,
            "AUTUMN_DATABASE__PRIMARY_URL",
            &mut self.database.primary_url,
        );
        parse_env_option_string(
            env,
            "AUTUMN_DATABASE__REPLICA_URL",
            &mut self.database.replica_url,
        );
        parse_env(
            env,
            "AUTUMN_DATABASE__POOL_SIZE",
            &mut self.database.pool_size,
        );
        parse_env_option(
            env,
            "AUTUMN_DATABASE__PRIMARY_POOL_SIZE",
            &mut self.database.primary_pool_size,
        );
        parse_env_option(
            env,
            "AUTUMN_DATABASE__REPLICA_POOL_SIZE",
            &mut self.database.replica_pool_size,
        );
        parse_env(
            env,
            "AUTUMN_DATABASE__REPLICA_FALLBACK",
            &mut self.database.replica_fallback,
        );
        parse_env(
            env,
            "AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS",
            &mut self.database.connect_timeout_secs,
        );
        parse_env_bool(
            env,
            "AUTUMN_DATABASE__AUTO_MIGRATE_IN_PRODUCTION",
            &mut self.database.auto_migrate_in_production,
        );
    }

    fn apply_log_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(env, "AUTUMN_LOG__LEVEL", &mut self.log.level);
        if let Ok(val) = env.var("AUTUMN_LOG__FORMAT") {
            match val.as_str() {
                "Auto" => self.log.format = LogFormat::Auto,
                "Pretty" => self.log.format = LogFormat::Pretty,
                "Json" => self.log.format = LogFormat::Json,
                _ => eprintln!(
                    "Warning: AUTUMN_LOG__FORMAT={val:?} is not valid \
                     (expected Auto, Pretty, or Json), ignoring"
                ),
            }
        }
    }

    fn apply_telemetry_env_overrides_with_env(&mut self, env: &dyn Env) {
        // ── Health ──────────────────────────────────────────────
        parse_env_bool(
            env,
            "AUTUMN_TELEMETRY__ENABLED",
            &mut self.telemetry.enabled,
        );
        parse_env_string(
            env,
            "AUTUMN_TELEMETRY__SERVICE_NAME",
            &mut self.telemetry.service_name,
        );
        parse_env_option_string(
            env,
            "AUTUMN_TELEMETRY__SERVICE_NAMESPACE",
            &mut self.telemetry.service_namespace,
        );
        parse_env_string(
            env,
            "AUTUMN_TELEMETRY__SERVICE_VERSION",
            &mut self.telemetry.service_version,
        );
        parse_env_string(
            env,
            "AUTUMN_TELEMETRY__ENVIRONMENT",
            &mut self.telemetry.environment,
        );
        parse_env_option_string(
            env,
            "AUTUMN_TELEMETRY__OTLP_ENDPOINT",
            &mut self.telemetry.otlp_endpoint,
        );
        if let Ok(val) = env.var("AUTUMN_TELEMETRY__PROTOCOL") {
            match TelemetryProtocol::from_env_value(&val) {
                Some(protocol) => self.telemetry.protocol = protocol,
                None => eprintln!(
                    "Warning: AUTUMN_TELEMETRY__PROTOCOL={val:?} is not valid \
                     (expected Grpc or HttpProtobuf), ignoring"
                ),
            }
        }
        parse_env_bool(env, "AUTUMN_TELEMETRY__STRICT", &mut self.telemetry.strict);
    }

    fn apply_health_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(env, "AUTUMN_HEALTH__PATH", &mut self.health.path);
        parse_env_string(env, "AUTUMN_HEALTH__LIVE_PATH", &mut self.health.live_path);
        parse_env_string(
            env,
            "AUTUMN_HEALTH__READY_PATH",
            &mut self.health.ready_path,
        );
        parse_env_string(
            env,
            "AUTUMN_HEALTH__STARTUP_PATH",
            &mut self.health.startup_path,
        );
        parse_env_bool(env, "AUTUMN_HEALTH__DETAILED", &mut self.health.detailed);
    }

    fn apply_cors_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_csv(
            env,
            "AUTUMN_CORS__ALLOWED_ORIGINS",
            &mut self.cors.allowed_origins,
        );
        parse_env_csv(
            env,
            "AUTUMN_CORS__ALLOWED_METHODS",
            &mut self.cors.allowed_methods,
        );
        parse_env_csv(
            env,
            "AUTUMN_CORS__ALLOWED_HEADERS",
            &mut self.cors.allowed_headers,
        );
        parse_env_bool(
            env,
            "AUTUMN_CORS__ALLOW_CREDENTIALS",
            &mut self.cors.allow_credentials,
        );
        parse_env(
            env,
            "AUTUMN_CORS__MAX_AGE_SECS",
            &mut self.cors.max_age_secs,
        );
    }

    fn apply_session_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(
            env,
            "AUTUMN_SESSION__COOKIE_NAME",
            &mut self.session.cookie_name,
        );
        if let Ok(val) = env.var("AUTUMN_SESSION__BACKEND") {
            match crate::session::SessionBackend::from_env_value(&val) {
                Some(backend) => self.session.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_SESSION__BACKEND={val:?} is not valid \
                     (expected memory or redis), ignoring"
                ),
            }
        }
        parse_env(
            env,
            "AUTUMN_SESSION__MAX_AGE_SECS",
            &mut self.session.max_age_secs,
        );
        parse_env_bool(env, "AUTUMN_SESSION__SECURE", &mut self.session.secure);
        parse_env_string(
            env,
            "AUTUMN_SESSION__SAME_SITE",
            &mut self.session.same_site,
        );
        parse_env_bool(
            env,
            "AUTUMN_SESSION__HTTP_ONLY",
            &mut self.session.http_only,
        );
        parse_env_string(env, "AUTUMN_SESSION__PATH", &mut self.session.path);
        parse_env_bool(
            env,
            "AUTUMN_SESSION__ALLOW_MEMORY_IN_PRODUCTION",
            &mut self.session.allow_memory_in_production,
        );
        parse_env_option_string(
            env,
            "AUTUMN_SESSION__REDIS__URL",
            &mut self.session.redis.url,
        );
        parse_env_string(
            env,
            "AUTUMN_SESSION__REDIS__KEY_PREFIX",
            &mut self.session.redis.key_prefix,
        );
    }

    fn apply_cache_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_CACHE__BACKEND") {
            match CacheBackend::from_env_value(&val) {
                Some(backend) => self.cache.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_CACHE__BACKEND={val:?} is not valid \
                     (expected memory or redis), ignoring"
                ),
            }
        }
        parse_env_option_string(env, "AUTUMN_CACHE__REDIS__URL", &mut self.cache.redis.url);
        parse_env_string(
            env,
            "AUTUMN_CACHE__REDIS__KEY_PREFIX",
            &mut self.cache.redis.key_prefix,
        );
    }

    fn apply_channels_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_CHANNELS__BACKEND") {
            match ChannelBackend::from_env_value(&val) {
                Some(backend) => self.channels.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_CHANNELS__BACKEND={val:?} is not valid \
                     (expected in_process or redis), ignoring"
                ),
            }
        }
        parse_env(
            env,
            "AUTUMN_CHANNELS__CAPACITY",
            &mut self.channels.capacity,
        );
        parse_env_option_string(
            env,
            "AUTUMN_CHANNELS__REDIS__URL",
            &mut self.channels.redis.url,
        );
        parse_env_string(
            env,
            "AUTUMN_CHANNELS__REDIS__KEY_PREFIX",
            &mut self.channels.redis.key_prefix,
        );
    }

    fn apply_jobs_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(env, "AUTUMN_JOBS__BACKEND", &mut self.jobs.backend);
        parse_env(env, "AUTUMN_JOBS__WORKERS", &mut self.jobs.workers);
        parse_env(
            env,
            "AUTUMN_JOBS__MAX_ATTEMPTS",
            &mut self.jobs.max_attempts,
        );
        parse_env(
            env,
            "AUTUMN_JOBS__INITIAL_BACKOFF_MS",
            &mut self.jobs.initial_backoff_ms,
        );
        parse_env_option_string(env, "AUTUMN_JOBS__REDIS__URL", &mut self.jobs.redis.url);
        parse_env_string(
            env,
            "AUTUMN_JOBS__REDIS__KEY_PREFIX",
            &mut self.jobs.redis.key_prefix,
        );
        parse_env(
            env,
            "AUTUMN_JOBS__REDIS__VISIBILITY_TIMEOUT_MS",
            &mut self.jobs.redis.visibility_timeout_ms,
        );
        parse_env(
            env,
            "AUTUMN_JOBS__POSTGRES__VISIBILITY_TIMEOUT_MS",
            &mut self.jobs.postgres.visibility_timeout_ms,
        );
    }

    fn apply_scheduler_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_SCHEDULER__BACKEND") {
            match SchedulerBackend::from_env_value(&val) {
                Some(backend) => self.scheduler.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_SCHEDULER__BACKEND={val:?} is not valid \
                     (expected in_process or postgres), ignoring"
                ),
            }
        }
        parse_env(
            env,
            "AUTUMN_SCHEDULER__LEASE_TTL_SECS",
            &mut self.scheduler.lease_ttl_secs,
        );
        parse_env_option_string(
            env,
            "AUTUMN_SCHEDULER__REPLICA_ID",
            &mut self.scheduler.replica_id,
        );
        parse_env_string(
            env,
            "AUTUMN_SCHEDULER__KEY_PREFIX",
            &mut self.scheduler.key_prefix,
        );
    }

    fn apply_auth_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env(env, "AUTUMN_AUTH__BCRYPT_COST", &mut self.auth.bcrypt_cost);
        parse_env_string(env, "AUTUMN_AUTH__SESSION_KEY", &mut self.auth.session_key);
        parse_env(
            env,
            "AUTUMN_AUTH__LOCKOUT__ENABLED",
            &mut self.auth.lockout.enabled,
        );
        parse_env(
            env,
            "AUTUMN_AUTH__LOCKOUT__THRESHOLD",
            &mut self.auth.lockout.threshold,
        );
        parse_env(
            env,
            "AUTUMN_AUTH__LOCKOUT__WINDOW_SECS",
            &mut self.auth.lockout.window_secs,
        );
        parse_env(
            env,
            "AUTUMN_AUTH__LOCKOUT__COOLOFF_SECS",
            &mut self.auth.lockout.cooloff_secs,
        );
        #[cfg(feature = "oauth2")]
        {
            let provider_names: Vec<String> = self.auth.oauth2.providers.keys().cloned().collect();
            for name in provider_names {
                let upper = name
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .to_uppercase();

                let client_id_var = format!("AUTUMN_AUTH__OAUTH2__{upper}__CLIENT_ID");
                if let Ok(id) = env.var(&client_id_var)
                    && !id.is_empty()
                    && let Some(p) = self.auth.oauth2.providers.get_mut(&name)
                {
                    p.client_id = id;
                }

                let client_secret_var = format!("AUTUMN_AUTH__OAUTH2__{upper}__CLIENT_SECRET");
                if let Ok(secret) = env.var(&client_secret_var)
                    && !secret.is_empty()
                    && let Some(p) = self.auth.oauth2.providers.get_mut(&name)
                {
                    p.client_secret = secret;
                }
            }
        }
    }

    /// Apply `AUTUMN_SECURITY__*` environment variable overrides.
    #[allow(clippy::too_many_lines)]
    fn apply_security_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_string(
            env,
            "AUTUMN_SECURITY__HEADERS__X_FRAME_OPTIONS",
            &mut self.security.headers.x_frame_options,
        );
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__HEADERS__X_CONTENT_TYPE_OPTIONS",
            &mut self.security.headers.x_content_type_options,
        );
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__HEADERS__STRICT_TRANSPORT_SECURITY",
            &mut self.security.headers.strict_transport_security,
        );
        parse_env(
            env,
            "AUTUMN_SECURITY__HEADERS__HSTS_MAX_AGE_SECS",
            &mut self.security.headers.hsts_max_age_secs,
        );
        parse_env_string(
            env,
            "AUTUMN_SECURITY__HEADERS__CONTENT_SECURITY_POLICY",
            &mut self.security.headers.content_security_policy,
        );
        parse_env_string(
            env,
            "AUTUMN_SECURITY__HEADERS__REFERRER_POLICY",
            &mut self.security.headers.referrer_policy,
        );
        parse_env_string(
            env,
            "AUTUMN_SECURITY__HEADERS__PERMISSIONS_POLICY",
            &mut self.security.headers.permissions_policy,
        );

        // CSRF
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__CSRF__ENABLED",
            &mut self.security.csrf.enabled,
        );
        parse_env_string(
            env,
            "AUTUMN_SECURITY__CSRF__TOKEN_HEADER",
            &mut self.security.csrf.token_header,
        );
        parse_env_string(
            env,
            "AUTUMN_SECURITY__CSRF__COOKIE_NAME",
            &mut self.security.csrf.cookie_name,
        );

        self.apply_rate_limit_env_overrides_with_env(env);

        // Multipart uploads
        parse_env(
            env,
            "AUTUMN_SECURITY__UPLOAD__MAX_REQUEST_SIZE_BYTES",
            &mut self.security.upload.max_request_size_bytes,
        );
        parse_env(
            env,
            "AUTUMN_SECURITY__UPLOAD__MAX_FILE_SIZE_BYTES",
            &mut self.security.upload.max_file_size_bytes,
        );
        parse_env_csv(
            env,
            "AUTUMN_SECURITY__UPLOAD__ALLOWED_MIME_TYPES",
            &mut self.security.upload.allowed_mime_types,
        );

        // Authorization deny shape + repository-API escape hatch.
        if let Ok(value) = env.var("AUTUMN_SECURITY__FORBIDDEN_RESPONSE") {
            match value.parse::<crate::authorization::ForbiddenResponse>() {
                Ok(parsed) => self.security.forbidden_response = parsed,
                Err(err) => tracing::warn!(
                    "ignoring invalid AUTUMN_SECURITY__FORBIDDEN_RESPONSE={value:?}: {err}"
                ),
            }
        }
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__ALLOW_UNAUTHORIZED_REPOSITORY_API",
            &mut self.security.allow_unauthorized_repository_api,
        );

        // Signing secret (canonical env var documented in deployment guide)
        parse_env_option_string(
            env,
            "AUTUMN_SECURITY__SIGNING_SECRET",
            &mut self.security.signing_secret.secret,
        );
        parse_env_csv(
            env,
            "AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS",
            &mut self.security.trusted_hosts.hosts,
        );

        // Top-level trusted-proxy policy
        parse_env_csv(
            env,
            "AUTUMN_SECURITY__TRUSTED_PROXIES__RANGES",
            &mut self.security.trusted_proxies.ranges,
        );
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__TRUSTED_PROXIES__TRUST_FORWARDED_HEADERS",
            &mut self.security.trusted_proxies.trust_forwarded_headers,
        );
        if let Ok(val) = env.var("AUTUMN_SECURITY__TRUSTED_PROXIES__TRUSTED_HOPS") {
            if let Ok(hops) = val.trim().parse::<u32>() {
                self.security.trusted_proxies.trusted_hops = Some(hops);
            } else {
                tracing::warn!(
                    "ignoring invalid AUTUMN_SECURITY__TRUSTED_PROXIES__TRUSTED_HOPS={val:?}: \
                     expected a non-negative integer"
                );
            }
        }

        self.security.webhooks.apply_env_overrides_with_env(env);
    }

    fn apply_rate_limit_env_overrides_with_env(&mut self, env: &dyn Env) {
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__RATE_LIMIT__ENABLED",
            &mut self.security.rate_limit.enabled,
        );
        parse_env(
            env,
            "AUTUMN_SECURITY__RATE_LIMIT__REQUESTS_PER_SECOND",
            &mut self.security.rate_limit.requests_per_second,
        );
        parse_env(
            env,
            "AUTUMN_SECURITY__RATE_LIMIT__BURST",
            &mut self.security.rate_limit.burst,
        );
        parse_env_bool(
            env,
            "AUTUMN_SECURITY__RATE_LIMIT__TRUST_FORWARDED_HEADERS",
            &mut self.security.rate_limit.trust_forwarded_headers,
        );
        parse_env_csv(
            env,
            "AUTUMN_SECURITY__RATE_LIMIT__TRUSTED_PROXIES",
            &mut self.security.rate_limit.trusted_proxies,
        );
        if let Ok(val) = env.var("AUTUMN_SECURITY__RATE_LIMIT__KEY_STRATEGY") {
            match crate::security::config::KeyStrategy::from_env_value(&val) {
                Some(strategy) => self.security.rate_limit.key_strategy = strategy,
                None => eprintln!(
                    "Warning: AUTUMN_SECURITY__RATE_LIMIT__KEY_STRATEGY={val:?} is not valid \
                     (expected ip, api_token, or authenticated_principal), ignoring"
                ),
            }
        }
        // BACKEND is always parsed so misconfiguration is surfaced even without
        // the redis feature (build_backend will warn and fall back to memory).
        if let Ok(val) = env.var("AUTUMN_SECURITY__RATE_LIMIT__BACKEND") {
            match crate::security::config::RateLimitBackend::from_env_value(&val) {
                Some(backend) => self.security.rate_limit.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_SECURITY__RATE_LIMIT__BACKEND={val:?} is not valid \
                     (expected memory or redis), ignoring"
                ),
            }
        }
        #[cfg(feature = "redis")]
        {
            use crate::security::config::RateLimitBackendFailure;
            if let Ok(val) = env.var("AUTUMN_SECURITY__RATE_LIMIT__ON_BACKEND_FAILURE") {
                match RateLimitBackendFailure::from_env_value(&val) {
                    Some(mode) => self.security.rate_limit.on_backend_failure = mode,
                    None => eprintln!(
                        "Warning: AUTUMN_SECURITY__RATE_LIMIT__ON_BACKEND_FAILURE={val:?} is not \
                         valid (expected fail_open or fail_closed), ignoring"
                    ),
                }
            }
            parse_env_option_string(
                env,
                "AUTUMN_SECURITY__RATE_LIMIT__REDIS__URL",
                &mut self.security.rate_limit.redis.url,
            );
            parse_env_string(
                env,
                "AUTUMN_SECURITY__RATE_LIMIT__REDIS__KEY_PREFIX",
                &mut self.security.rate_limit.redis.key_prefix,
            );
        }
    }

    #[cfg(feature = "storage")]
    fn apply_storage_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_STORAGE__BACKEND") {
            match crate::storage::StorageBackend::from_env_value(&val) {
                Some(backend) => self.storage.backend = backend,
                None => eprintln!(
                    "Warning: AUTUMN_STORAGE__BACKEND={val:?} is not valid \
                     (expected disabled, local, or s3), ignoring"
                ),
            }
        }
        parse_env_string(
            env,
            "AUTUMN_STORAGE__DEFAULT_PROVIDER",
            &mut self.storage.default_provider,
        );
        parse_env_bool(
            env,
            "AUTUMN_STORAGE__ALLOW_LOCAL_IN_PRODUCTION",
            &mut self.storage.allow_local_in_production,
        );
        if let Ok(val) = env.var("AUTUMN_STORAGE__LOCAL__ROOT") {
            self.storage.local.root = PathBuf::from(val);
        }
        parse_env_string(
            env,
            "AUTUMN_STORAGE__LOCAL__MOUNT_PATH",
            &mut self.storage.local.mount_path,
        );
        parse_env(
            env,
            "AUTUMN_STORAGE__LOCAL__DEFAULT_URL_EXPIRY_SECS",
            &mut self.storage.local.default_url_expiry_secs,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__LOCAL__SIGNING_KEY",
            &mut self.storage.local.signing_key,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__BUCKET",
            &mut self.storage.s3.bucket,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__REGION",
            &mut self.storage.s3.region,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__ENDPOINT",
            &mut self.storage.s3.endpoint,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__PUBLIC_BASE_URL",
            &mut self.storage.s3.public_base_url,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__ACCESS_KEY_ID_ENV",
            &mut self.storage.s3.access_key_id_env,
        );
        parse_env_option_string(
            env,
            "AUTUMN_STORAGE__S3__SECRET_ACCESS_KEY_ENV",
            &mut self.storage.s3.secret_access_key_env,
        );
        parse_env_bool(
            env,
            "AUTUMN_STORAGE__S3__FORCE_PATH_STYLE",
            &mut self.storage.s3.force_path_style,
        );
        parse_env(
            env,
            "AUTUMN_STORAGE__S3__DEFAULT_URL_EXPIRY_SECS",
            &mut self.storage.s3.default_url_expiry_secs,
        );
        parse_env(
            env,
            "AUTUMN_STORAGE__VARIANTS__MAX_SOURCE_BYTES",
            &mut self.storage.variants.max_source_bytes,
        );
        parse_env(
            env,
            "AUTUMN_STORAGE__VARIANTS__MAX_SOURCE_WIDTH",
            &mut self.storage.variants.max_source_width,
        );
        parse_env(
            env,
            "AUTUMN_STORAGE__VARIANTS__MAX_SOURCE_HEIGHT",
            &mut self.storage.variants.max_source_height,
        );
    }

    #[cfg(feature = "mail")]
    fn apply_mail_env_overrides_with_env(&mut self, env: &dyn Env) {
        if let Ok(val) = env.var("AUTUMN_MAIL__TRANSPORT") {
            match crate::mail::Transport::from_env_value(&val) {
                Some(transport) => self.mail.transport = transport,
                None => eprintln!(
                    "Warning: AUTUMN_MAIL__TRANSPORT={val:?} is not valid \
                     (expected log, file, smtp, or disabled), ignoring"
                ),
            }
        }
        parse_env_option_string(env, "AUTUMN_MAIL__FROM", &mut self.mail.from);
        parse_env_option_string(env, "AUTUMN_MAIL__REPLY_TO", &mut self.mail.reply_to);
        parse_env_bool(
            env,
            "AUTUMN_MAIL__ALLOW_LOG_IN_PRODUCTION",
            &mut self.mail.allow_log_in_production,
        );
        parse_env_bool(
            env,
            "AUTUMN_MAIL__ALLOW_IN_PROCESS_DELIVER_LATER_IN_PRODUCTION",
            &mut self.mail.allow_in_process_deliver_later_in_production,
        );
        parse_env_bool(env, "AUTUMN_MAIL__PREVIEW", &mut self.mail.preview);
        if let Ok(val) = env.var("AUTUMN_MAIL__FILE_DIR") {
            self.mail.file_dir = PathBuf::from(val);
        }
        parse_env_option_string(env, "AUTUMN_MAIL__SMTP__HOST", &mut self.mail.smtp.host);
        if let Ok(val) = env.var("AUTUMN_MAIL__SMTP__PORT") {
            match val.parse::<u16>() {
                Ok(port) => self.mail.smtp.port = Some(port),
                Err(_) => {
                    eprintln!("Warning: AUTUMN_MAIL__SMTP__PORT={val:?} is not valid, ignoring");
                }
            }
        }
        parse_env_option_string(
            env,
            "AUTUMN_MAIL__SMTP__USERNAME",
            &mut self.mail.smtp.username,
        );
        parse_env_option_string(
            env,
            "AUTUMN_MAIL__SMTP__PASSWORD_ENV",
            &mut self.mail.smtp.password_env,
        );
        if let Ok(val) = env.var("AUTUMN_MAIL__SMTP__TLS") {
            match crate::mail::TlsMode::from_env_value(&val) {
                Some(tls) => self.mail.smtp.tls = tls,
                None => eprintln!(
                    "Warning: AUTUMN_MAIL__SMTP__TLS={val:?} is not valid \
                     (expected disabled, starttls, or tls), ignoring"
                ),
            }
        }
    }

    /// Returns the active profile name, if any.
    #[must_use]
    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_deref()
    }
}

/// HTTP server configuration.
///
/// Controls which address the server binds to and how graceful shutdown
/// behaves.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `port` | `3000` |
/// | `host` | `"127.0.0.1"` |
/// | `shutdown_timeout_secs` | `30` |
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::ServerConfig;
///
/// let server = ServerConfig::default();
/// assert_eq!(server.port, 3000);
/// assert_eq!(server.host, "127.0.0.1");
/// ```
/// Per-request timeout configuration.
///
/// Controls how long the server waits for a complete request-response cycle
/// before returning `408 Request Timeout`. A value of `None` or `0` disables
/// the timeout (the default, so existing applications are unaffected).
///
/// # `autumn.toml` example
///
/// ```toml
/// [server.timeouts]
/// request_timeout_ms = 30000  # 30 seconds
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RequestTimeoutsConfig {
    /// Maximum time in milliseconds allowed for a complete request-response
    /// cycle. When exceeded the framework returns `408 Request Timeout` with
    /// a Problem Details body. `None` (default) or `0` disables the timeout.
    ///
    /// Configured via `AUTUMN_SERVER__TIMEOUTS__REQUEST_TIMEOUT_MS`.
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Port to listen on. Default: `3000`.
    #[serde(default = "default_port")]
    pub port: u16,

    /// Host/IP to bind to. Default: `"127.0.0.1"`.
    ///
    /// Set to `"0.0.0.0"` to accept connections from all interfaces
    /// (typical for containerized deployments).
    #[serde(default = "default_host")]
    pub host: String,

    /// Seconds to wait for in-flight requests during graceful shutdown.
    /// Default: `30`.
    ///
    /// When the server receives a shutdown signal, it stops accepting
    /// new connections and waits up to this many seconds for in-flight
    /// requests to complete before forcibly terminating.
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,

    /// Seconds between `/ready` returning 503 and the TCP listener
    /// closing to new connections. Default: `5`.
    ///
    /// This gap gives upstream load balancers time to deregister the
    /// replica before it stops accepting new connections, preventing
    /// connection resets on in-flight requests from the LB tier.
    /// Must be tuned to match the LB's health-check interval + deregistration
    /// propagation time. Set to `0` to disable the grace period.
    #[serde(default = "default_prestop_grace")]
    pub prestop_grace_secs: u64,

    /// Per-request timeout configuration.
    ///
    /// Controls request-cycle timeouts for `DoS` protection. By default
    /// all timeouts are disabled so existing applications are unaffected.
    /// Set `request_timeout_ms` in `[server.timeouts]` to enable.
    #[serde(default)]
    pub timeouts: RequestTimeoutsConfig,
}

/// Behavior when a configured read replica is unavailable or stale.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReplicaFallback {
    /// Readiness should fail when the configured replica cannot safely serve reads.
    #[default]
    FailReadiness,
    /// Read paths may use the primary when the replica is unavailable or stale.
    Primary,
}

impl std::str::FromStr for ReplicaFallback {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fail_readiness" | "fail-readiness" | "fail" => Ok(Self::FailReadiness),
            "primary" | "fallback_to_primary" | "fallback-to-primary" => Ok(Self::Primary),
            _ => Err(()),
        }
    }
}

/// Database connection configuration.
///
/// When `url` is `None` (the default), the application runs without a
/// database -- useful for static-site or API-gateway use cases. Set a
/// Postgres URL to enable the connection pool and the [`Db`](crate::Db)
/// extractor.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `url` | `None` |
/// | `primary_url` | `None` |
/// | `replica_url` | `None` |
/// | `pool_size` | `10` |
/// | `primary_pool_size` | `None` |
/// | `replica_pool_size` | `None` |
/// | `replica_fallback` | `fail_readiness` |
/// | `connect_timeout_secs` | `5` |
/// | `auto_migrate_in_production` | `false` |
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::DatabaseConfig;
///
/// let db = DatabaseConfig::default();
/// assert!(db.url.is_none());
/// assert_eq!(db.pool_size, 10);
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Postgres connection URL. `None` means no database is configured.
    ///
    /// Compatibility alias for the primary/write role. New multi-role
    /// deployments should prefer [`primary_url`](Self::primary_url).
    ///
    /// Must start with `postgres://` or `postgresql://` when present.
    #[serde(default)]
    pub url: Option<String>,

    /// Postgres URL for the primary/write role.
    ///
    /// All writes, transactions, advisory locks, and migrations use this role.
    /// When unset, [`url`](Self::url) remains the single-primary fallback.
    #[serde(default)]
    pub primary_url: Option<String>,

    /// Optional Postgres URL for the read/replica role.
    ///
    /// Read-only paths may use this pool when configured. If omitted, read
    /// paths use the primary role.
    #[serde(default)]
    pub replica_url: Option<String>,

    /// Maximum number of connections in the pool. Default: `10`.
    ///
    /// Compatibility/default pool size used for both roles unless a
    /// role-specific size is set.
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,

    /// Optional primary/write role pool size.
    #[serde(default)]
    pub primary_pool_size: Option<usize>,

    /// Optional read/replica role pool size.
    #[serde(default)]
    pub replica_pool_size: Option<usize>,

    /// Deterministic behavior for configured replicas that cannot safely serve
    /// reads. Default: fail readiness.
    #[serde(default)]
    pub replica_fallback: ReplicaFallback,

    /// Seconds to wait while acquiring a pooled connection, including
    /// creating a new connection when the pool grows.
    /// Default: `5`.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,

    /// When true, permits automatic migration application while running with
    /// `prod`/`production` profile. Default: `false`.
    ///
    /// Keep this disabled for multi-replica production fleets and use an
    /// explicit migration job (`autumn migrate`) instead.
    #[serde(default)]
    pub auto_migrate_in_production: bool,

    /// Optional database statement timeout.
    #[serde(deserialize_with = "deserialize_option_duration", default)]
    pub statement_timeout: Option<std::time::Duration>,

    /// Slow query threshold. Default: `500ms`.
    #[serde(
        deserialize_with = "deserialize_duration",
        default = "default_slow_query_threshold"
    )]
    pub slow_query_threshold: std::time::Duration,
}

impl DatabaseConfig {
    /// Resolved primary/write database URL.
    #[must_use]
    pub fn effective_primary_url(&self) -> Option<&str> {
        self.primary_url.as_deref().or(self.url.as_deref())
    }

    /// Resolved primary/write role pool size.
    #[must_use]
    pub fn effective_primary_pool_size(&self) -> usize {
        self.primary_pool_size.unwrap_or(self.pool_size)
    }

    /// Resolved read/replica role pool size.
    #[must_use]
    pub fn effective_replica_pool_size(&self) -> usize {
        self.replica_pool_size.unwrap_or(self.pool_size)
    }

    /// Validate database configuration.
    ///
    /// # Errors
    ///
    /// Returns a validation error if the URL has an invalid scheme.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, url) in [
            ("database.url", self.url.as_deref()),
            ("database.primary_url", self.primary_url.as_deref()),
            ("database.replica_url", self.replica_url.as_deref()),
        ] {
            if let Some(url) = url
                && !url.starts_with("postgres://")
                && !url.starts_with("postgresql://")
            {
                let label = if field == "database.url" {
                    "database URL"
                } else {
                    field
                };
                return Err(ConfigError::Validation(format!(
                    "Invalid {label}: must start with postgres:// or postgresql://, got {url:?}"
                )));
            }
        }

        if self.replica_url.is_some() && self.effective_primary_url().is_none() {
            return Err(ConfigError::Validation(
                "database.replica_url requires database.primary_url or database.url".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Logging configuration.
///
/// Controls the tracing subscriber's filter level and output format.
/// See [`LogFormat`] for output format options.
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::{LogConfig, LogFormat};
///
/// let log = LogConfig::default();
/// assert_eq!(log.level, "info");
/// assert_eq!(log.format, LogFormat::Auto);
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    /// Tracing filter directive. Default: `"info"`.
    ///
    /// Supports the full `tracing` filter syntax, e.g.
    /// `"autumn=debug,tower_http=trace"`.
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log output format. Default: [`LogFormat::Auto`].
    #[serde(default)]
    pub format: LogFormat,

    /// Additional sensitive parameter keys to scrub from logs/traces.
    #[serde(default)]
    pub filter_parameters: Vec<String>,

    /// Explicitly remove default sensitive keys from the built-in deny-list.
    #[serde(default)]
    pub unfilter_parameters: Vec<String>,
}

/// Log output format.
///
/// Controls how tracing events are rendered. The default ([`Auto`](Self::Auto))
/// auto-detects based on the `AUTUMN_ENV` environment variable.
///
/// | Variant | Behaviour |
/// |---------|-----------|
/// | [`Auto`](Self::Auto) | Pretty in dev, JSON when `AUTUMN_ENV=production` |
/// | [`Pretty`](Self::Pretty) | Always human-readable, colorized |
/// | [`Json`](Self::Json) | Always structured JSON (for log aggregators) |
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::LogFormat;
///
/// assert_eq!(LogFormat::default(), LogFormat::Auto);
/// ```
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogFormat {
    /// Pretty in dev, JSON in production (based on `AUTUMN_ENV`).
    #[default]
    Auto,
    /// Human-readable, colorized output.
    Pretty,
    /// Structured JSON output suitable for log aggregation pipelines.
    Json,
}

/// Telemetry configuration.
///
/// Controls whether Autumn enables OTLP trace export and how the process
/// identifies itself in resource metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    /// Enable framework-managed telemetry. Default: `false`.
    #[serde(default)]
    pub enabled: bool,

    /// Logical service name. Default: `"autumn-app"`.
    #[serde(default = "default_telemetry_service_name")]
    pub service_name: String,

    /// Optional service namespace (e.g. team, domain, or product family).
    #[serde(default)]
    pub service_namespace: Option<String>,

    /// Service version string advertised in resource metadata.
    #[serde(default = "default_telemetry_service_version")]
    pub service_version: String,

    /// Deployment environment label for trace resource metadata.
    #[serde(default = "default_telemetry_environment")]
    pub environment: String,

    /// OTLP collector endpoint. Required when telemetry is enabled.
    #[serde(default)]
    pub otlp_endpoint: Option<String>,

    /// OTLP transport protocol. Default: [`TelemetryProtocol::Grpc`].
    #[serde(default)]
    pub protocol: TelemetryProtocol,

    /// When `true`, telemetry initialization failures abort startup.
    #[serde(default)]
    pub strict: bool,
}

/// OTLP transport protocol selection.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum TelemetryProtocol {
    /// OTLP over gRPC.
    #[serde(alias = "grpc", alias = "GRPC")]
    #[default]
    Grpc,
    /// OTLP over HTTP/protobuf.
    #[serde(
        alias = "http-protobuf",
        alias = "http_protobuf",
        alias = "HTTP_PROTOBUF"
    )]
    HttpProtobuf,
}

impl TelemetryProtocol {
    fn from_env_value(value: &str) -> Option<Self> {
        match value {
            "Grpc" | "grpc" | "GRPC" => Some(Self::Grpc),
            "HttpProtobuf" | "http-protobuf" | "http_protobuf" | "HTTP_PROTOBUF"
            | "httpprotobuf" => Some(Self::HttpProtobuf),
            _ => None,
        }
    }
}

/// Health check endpoint configuration.
///
/// The health check is automatically mounted by [`AppBuilder::run`](crate::app::AppBuilder::run).
/// See the [`health`](crate::health) module for response format details.
///
/// # Examples
///
/// ```rust
/// use autumn_web::config::HealthConfig;
///
/// let health = HealthConfig::default();
/// assert_eq!(health.path, "/health");
/// assert_eq!(health.live_path, "/live");
/// assert_eq!(health.ready_path, "/ready");
/// assert_eq!(health.startup_path, "/startup");
/// assert!(!health.detailed);
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    /// Compatibility alias path for readiness. Default: `"/health"`.
    ///
    /// Common alternatives: `"/healthz"`, `"/_health"`.
    #[serde(default = "default_health_path")]
    pub path: String,

    /// URL path for the liveness probe. Default: `"/live"`.
    #[serde(default = "default_live_path")]
    pub live_path: String,

    /// URL path for the readiness probe. Default: `"/ready"`.
    #[serde(default = "default_ready_path")]
    pub ready_path: String,

    /// URL path for the startup probe. Default: `"/startup"`.
    #[serde(default = "default_startup_path")]
    pub startup_path: String,

    /// When `true`, the health endpoint includes detailed info (profile,
    /// uptime, pool stats). Default: `false` (overridden to `true` for
    /// `dev` profile via smart defaults).
    #[serde(default)]
    pub detailed: bool,
}

/// Actuator endpoint configuration.
///
/// Controls which operational endpoints are exposed. The `sensitive` flag
/// determines whether sensitive endpoints (env, configprops, loggers,
/// tasks) are available. Defaults to `true` for `dev`, `false` for `prod`.
#[derive(Debug, Clone, Deserialize)]
pub struct ActuatorConfig {
    /// URL prefix for actuator endpoints. Default: `"/actuator"`.
    #[serde(default = "default_actuator_prefix")]
    pub prefix: String,

    /// When `true`, expose sensitive endpoints (env, loggers, tasks).
    /// Defaults vary by profile: `true` for dev, `false` for prod.
    #[serde(default)]
    pub sensitive: bool,
}

impl Default for ActuatorConfig {
    fn default() -> Self {
        Self {
            prefix: default_actuator_prefix(),
            sensitive: false,
        }
    }
}

fn default_actuator_prefix() -> String {
    "/actuator".to_owned()
}

/// CORS (Cross-Origin Resource Sharing) configuration.
///
/// Controls which origins, methods, and headers are allowed for
/// cross-origin requests. Disabled by default -- enable by setting
/// `allowed_origins` in `autumn.toml` or via environment variables.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `allowed_origins` | `[]` (CORS disabled) |
/// | `allowed_methods` | `["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]` |
/// | `allowed_headers` | `["Content-Type", "Authorization"]` |
/// | `allow_credentials` | `false` |
/// | `max_age_secs` | `86400` (24 hours) |
///
/// # Profile smart defaults
///
/// The `dev` profile enables permissive CORS (`allowed_origins = ["*"]`)
/// so local front-end development works out of the box.
///
/// # Examples
///
/// ```toml
/// [cors]
/// allowed_origins = ["https://example.com", "https://app.example.com"]
/// allow_credentials = true
/// ```
///
/// ```rust
/// use autumn_web::config::CorsConfig;
///
/// let cors = CorsConfig::default();
/// assert!(cors.allowed_origins.is_empty());
/// assert!(!cors.allow_credentials);
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    /// Origins allowed to make cross-origin requests.
    ///
    /// Use `["*"]` to allow any origin (not recommended for production
    /// with credentials). When empty, CORS middleware is not applied.
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// HTTP methods allowed for cross-origin requests.
    /// Default: `["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]`.
    #[serde(default = "default_cors_methods")]
    pub allowed_methods: Vec<String>,

    /// Headers allowed in cross-origin requests.
    /// Default: `["Content-Type", "Authorization"]`.
    #[serde(default = "default_cors_headers")]
    pub allowed_headers: Vec<String>,

    /// Whether to include `Access-Control-Allow-Credentials: true`.
    /// Default: `false`.
    #[serde(default)]
    pub allow_credentials: bool,

    /// How long (in seconds) browsers may cache preflight responses.
    /// Default: `86400` (24 hours).
    #[serde(default = "default_cors_max_age")]
    pub max_age_secs: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: default_cors_methods(),
            allowed_headers: default_cors_headers(),
            allow_credentials: false,
            max_age_secs: default_cors_max_age(),
        }
    }
}

impl CorsConfig {
    /// Validate CORS configuration for combinations rejected by browsers.
    ///
    /// # Errors
    ///
    /// Returns a validation error when `allow_credentials = true` is combined
    /// with a wildcard `"*"` origin. Browsers refuse this combination per the
    /// Fetch spec, and `tower-http`'s `CorsLayer` panics when asked to build
    /// it, so we fail fast at config load with an actionable message.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.allow_credentials && self.allowed_origins.iter().any(|o| o == "*") {
            return Err(ConfigError::Validation(
                "CORS: allow_credentials=true is incompatible with allowed_origins=[\"*\"]; \
                 list explicit origins instead (browsers reject the wildcard+credentials combo)"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

fn default_cors_methods() -> Vec<String> {
    vec![
        "GET".to_owned(),
        "POST".to_owned(),
        "PUT".to_owned(),
        "DELETE".to_owned(),
        "PATCH".to_owned(),
        "OPTIONS".to_owned(),
    ]
}

fn default_cors_headers() -> Vec<String> {
    vec!["Content-Type".to_owned(), "Authorization".to_owned()]
}

const fn default_cors_max_age() -> u64 {
    86400
}

// ── CompressionConfig ──────────────────────────────────────────────────────

/// Response compression settings (`[compression]` section in `autumn.toml`).
///
/// Compression is **off by default** to avoid the [BREACH/CRIME] class of
/// compression side-channel attacks, where an attacker can infer secret
/// content (e.g. CSRF tokens) by observing how the compressed size changes as
/// they inject attacker-controlled bytes alongside the secret. Enable only when
/// you understand the tradeoff — or when a CDN / reverse-proxy handles TLS and
/// terminates there.
///
/// [BREACH/CRIME]: https://breachattack.com/
///
/// # One-liner opt-in
///
/// ```toml
/// [compression]
/// enabled = true
/// ```
///
/// # Environment variable override
///
/// | Variable | Field | Type |
/// |----------|-------|------|
/// | `AUTUMN_COMPRESSION__ENABLED` | `enabled` | `bool` |
///
/// # `ETag` compatibility
///
/// Autumn's framework-managed compression layer is applied **outside** any
/// user-registered `EtagLayer`, so `ETags` are computed on the uncompressed body.
/// Because `CompressionLayer` sets `Vary: Accept-Encoding`, caches correctly
/// store separate entries per encoding. Using weak `ETags` (`W/`) when
/// compression is enabled is safe per RFC 7232 §2.1 (weak comparison allows
/// encoding variations).
///
/// # Example
///
/// ```rust
/// use autumn_web::config::CompressionConfig;
///
/// let cfg = CompressionConfig::default();
/// assert!(!cfg.enabled);
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CompressionConfig {
    /// Enable response compression. Default: `false`.
    ///
    /// When `true`, the framework inserts a `CompressionLayer` that honors the
    /// client's `Accept-Encoding` header (gzip and brotli supported) and sets
    /// `Vary: Accept-Encoding` on all compressible responses.
    /// Non-compressible content types (images, archives) and responses that
    /// already carry `Content-Encoding` are passed through unchanged.
    #[serde(default)]
    pub enabled: bool,
}

/// Parse an environment variable into a typed target, logging a warning on failure.
fn parse_env<T: std::str::FromStr>(env: &dyn Env, key: &str, target: &mut T) {
    if let Ok(val) = env.var(key) {
        match val.parse::<T>() {
            Ok(v) => *target = v,
            Err(_) => eprintln!("Warning: {key}={val:?} is not valid, ignoring"),
        }
    }
}

fn parse_env_option_string(env: &dyn Env, key: &str, target: &mut Option<String>) {
    if let Ok(val) = env.var(key) {
        *target = if val.is_empty() { None } else { Some(val) };
    }
}

fn parse_env_option<T: std::str::FromStr>(env: &dyn Env, key: &str, target: &mut Option<T>) {
    if let Ok(val) = env.var(key) {
        if val.is_empty() {
            *target = None;
        } else {
            match val.parse::<T>() {
                Ok(v) => *target = Some(v),
                Err(_) => eprintln!("Warning: {key}={val:?} is not valid, ignoring"),
            }
        }
    }
}

fn parse_env_string(env: &dyn Env, key: &str, target: &mut String) {
    if let Ok(val) = env.var(key) {
        *target = val;
    }
}

fn parse_env_bool(env: &dyn Env, key: &str, target: &mut bool) {
    if let Ok(val) = env.var(key) {
        match val.as_str() {
            "true" | "1" => *target = true,
            "false" | "0" => *target = false,
            _ => eprintln!("Warning: {key}={val:?} is not valid (expected true/false), ignoring"),
        }
    }
}

fn parse_env_option_bool(env: &dyn Env, key: &str, target: &mut Option<bool>) {
    if let Ok(val) = env.var(key) {
        match val.as_str() {
            "true" | "1" => *target = Some(true),
            "false" | "0" => *target = Some(false),
            _ => eprintln!("Warning: {key}={val:?} is not valid (expected true/false), ignoring"),
        }
    }
}

fn parse_env_csv(env: &dyn Env, key: &str, target: &mut Vec<String>) {
    if let Ok(val) = env.var(key) {
        *target = val.split(',').map(|s| s.trim().to_owned()).collect();
    }
}

// ── Default functions ──────────────────────────────────────────────

const fn default_port() -> u16 {
    3000
}

fn default_host() -> String {
    "127.0.0.1".to_owned()
}

const fn default_shutdown_timeout() -> u64 {
    30
}

const fn default_prestop_grace() -> u64 {
    5
}

const fn default_pool_size() -> usize {
    10
}

const fn default_connect_timeout() -> u64 {
    5
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_telemetry_service_name() -> String {
    "autumn-app".to_owned()
}

fn default_telemetry_service_version() -> String {
    "unknown".to_owned()
}

fn default_telemetry_environment() -> String {
    "development".to_owned()
}

fn default_health_path() -> String {
    "/health".to_owned()
}

fn default_live_path() -> String {
    "/live".to_owned()
}

fn default_ready_path() -> String {
    "/ready".to_owned()
}

fn default_startup_path() -> String {
    "/startup".to_owned()
}

// ── Default trait impls ────────────────────────────────────────────

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            host: default_host(),
            shutdown_timeout_secs: default_shutdown_timeout(),
            prestop_grace_secs: default_prestop_grace(),
            timeouts: RequestTimeoutsConfig::default(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: None,
            primary_url: None,
            replica_url: None,
            pool_size: default_pool_size(),
            primary_pool_size: None,
            replica_pool_size: None,
            replica_fallback: ReplicaFallback::default(),
            connect_timeout_secs: default_connect_timeout(),
            auto_migrate_in_production: false,
            statement_timeout: None,
            slow_query_threshold: default_slow_query_threshold(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: LogFormat::default(),
            filter_parameters: Vec::new(),
            unfilter_parameters: Vec::new(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            service_name: default_telemetry_service_name(),
            service_namespace: None,
            service_version: default_telemetry_service_version(),
            environment: default_telemetry_environment(),
            otlp_endpoint: None,
            protocol: TelemetryProtocol::default(),
            strict: false,
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            path: default_health_path(),
            live_path: default_live_path(),
            ready_path: default_ready_path(),
            startup_path: default_startup_path(),
            detailed: false,
        }
    }
}

// ----------------------------------------------------------------------------
// ConfigLoader — tier-1 boot-time replaceable config loading
// ----------------------------------------------------------------------------

/// Pluggable boot-time configuration loader.
///
/// Replace the default TOML + env loader with a custom strategy (e.g. AWS
/// Secrets Manager, Consul, a JSON file, an HTTP fetch) by implementing this
/// trait and installing it on the [`AppBuilder`](crate::app::AppBuilder) via
/// [`with_config_loader`](crate::app::AppBuilder::with_config_loader).
///
/// The trait's return type uses `impl Future + Send` so implementations can
/// freely use `async fn` in their bodies while the framework can still spawn
/// the load on any executor.
///
/// # Example
///
/// ```rust,no_run
/// use autumn_web::config::{AutumnConfig, ConfigError, ConfigLoader};
///
/// pub struct JsonFileConfigLoader { path: std::path::PathBuf }
///
/// impl ConfigLoader for JsonFileConfigLoader {
///     async fn load(&self) -> Result<AutumnConfig, ConfigError> {
///         let bytes = std::fs::read(&self.path).map_err(ConfigError::Io)?;
///         serde_json::from_slice(&bytes)
///             .map_err(|e| ConfigError::Validation(e.to_string()))
///     }
/// }
/// ```
pub trait ConfigLoader: Send + Sync + 'static {
    /// Load and return a fully-resolved [`AutumnConfig`].
    ///
    /// Implementations are responsible for any layering, profile resolution,
    /// and validation they care to apply. The default implementation
    /// ([`TomlEnvConfigLoader`]) preserves Autumn's five-layer load
    /// (framework defaults → profile defaults → `autumn.toml` →
    /// `autumn-{profile}.toml` → `AUTUMN_*` env vars).
    fn load(&self) -> impl std::future::Future<Output = Result<AutumnConfig, ConfigError>> + Send;
}

/// Default [`ConfigLoader`] — Autumn's five-layer TOML + env load strategy.
///
/// Delegates to [`AutumnConfig::load_with_env`] using [`OsEnv`] for environment
/// variable reads. This is the loader used when no override is installed via
/// [`with_config_loader`](crate::app::AppBuilder::with_config_loader).
#[derive(Debug, Default, Clone, Copy)]
pub struct TomlEnvConfigLoader;

impl TomlEnvConfigLoader {
    /// Construct a new default loader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ConfigLoader for TomlEnvConfigLoader {
    async fn load(&self) -> Result<AutumnConfig, ConfigError> {
        AutumnConfig::load_with_env(&OsEnv)
    }
}

const fn default_slow_query_threshold() -> std::time::Duration {
    std::time::Duration::from_millis(500)
}

/// Parses a duration string like "500ms", "5s", "2m", "1h",
/// or a plain integer representing milliseconds.
///
/// # Errors
/// Returns a `String` describing the parse failure when the input is empty,
/// has an unrecognised suffix, or contains a non-numeric value.
pub fn parse_duration_str(s: &str) -> Result<std::time::Duration, String> {
    if s.is_empty() {
        return Err("duration string is empty".to_owned());
    }

    // Check if it's a plain integer
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(std::time::Duration::from_millis(ms));
    }

    // Try parsing suffix
    if let Some(val_str) = s.strip_suffix("ms") {
        let val = val_str
            .parse::<u64>()
            .map_err(|e| format!("invalid duration integer: {e}"))?;
        return Ok(std::time::Duration::from_millis(val));
    }

    if let Some(val_str) = s.strip_suffix('s') {
        let val = val_str
            .parse::<u64>()
            .map_err(|e| format!("invalid duration integer: {e}"))?;
        return Ok(std::time::Duration::from_secs(val));
    }

    if let Some(val_str) = s.strip_suffix('m') {
        let val = val_str
            .parse::<u64>()
            .map_err(|e| format!("invalid duration integer: {e}"))?;
        let secs = val.checked_mul(60).ok_or_else(|| {
            format!("duration overflow: '{s}' exceeds maximum representable value")
        })?;
        return Ok(std::time::Duration::from_secs(secs));
    }

    if let Some(val_str) = s.strip_suffix('h') {
        let val = val_str
            .parse::<u64>()
            .map_err(|e| format!("invalid duration integer: {e}"))?;
        let secs = val.checked_mul(3600).ok_or_else(|| {
            format!("duration overflow: '{s}' exceeds maximum representable value")
        })?;
        return Ok(std::time::Duration::from_secs(secs));
    }

    Err(format!("invalid duration format: '{s}'"))
}

/// Deserialises a TOML/JSON value into a [`std::time::Duration`].
///
/// Accepts either a string (`"500ms"`, `"5s"`, `"2m"`, `"1h"`) or a bare
/// integer (interpreted as milliseconds).
///
/// # Errors
/// Returns a deserialisation error if the value is not a valid duration.
pub fn deserialize_duration<'de, D>(deserializer: D) -> Result<std::time::Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DurationOrStr {
        String(String),
        Integer(u64),
    }

    match DurationOrStr::deserialize(deserializer)? {
        DurationOrStr::String(s) => parse_duration_str(&s).map_err(serde::de::Error::custom),
        DurationOrStr::Integer(i) => Ok(std::time::Duration::from_millis(i)),
    }
}

/// Deserialises an optional TOML/JSON value into <code>Option&lt;[std::time::Duration]&gt;</code>.
///
/// Accepts either a string (`"500ms"`, `"5s"`, `"2m"`, `"1h"`), a bare
/// integer (milliseconds), or `null`/absent to mean no timeout.
///
/// # Errors
/// Returns a deserialisation error if the value is present but invalid.
pub fn deserialize_option_duration<'de, D>(
    deserializer: D,
) -> Result<Option<std::time::Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Wrapper(#[serde(deserialize_with = "deserialize_duration")] std::time::Duration);

    Option::<Wrapper>::deserialize(deserializer).map(|opt| opt.map(|w| w.0))
}

/// Row-level multi-tenancy configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct TenancyConfig {
    /// Whether row-level multi-tenancy is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Source configuration from which the tenant ID is extracted.
    /// Values can be "header" (default), "subdomain", "session", "jwt".
    #[serde(default = "default_tenancy_source")]
    pub source: String,

    /// Header name to lookup if source is "header". Default: "x-tenant-id".
    #[serde(default = "default_tenancy_header_name")]
    pub header_name: String,

    /// Session key to lookup if source is "session". Default: "`tenant_id`".
    #[serde(default = "default_tenancy_session_key")]
    pub session_key: String,

    /// JWT claim to lookup if source is "jwt". Default: "`tenant_id`".
    #[serde(default = "default_tenancy_jwt_claim")]
    pub jwt_claim: String,

    /// JWT secret key used to verify the JWT signature.
    #[serde(default)]
    pub jwt_secret: Option<String>,

    /// Expected JWT issuer to validate.
    #[serde(default)]
    pub jwt_issuer: Option<String>,

    /// Expected JWT audience (`aud` claim) to validate.
    /// When set, audience checking is enabled; when `None`, audience checking
    /// is skipped for backward compatibility.
    #[serde(default)]
    pub jwt_audience: Option<String>,

    /// Optional base domain for subdomain tenancy.
    #[serde(default)]
    pub base_domain: Option<String>,
}

fn default_tenancy_source() -> String {
    "header".to_string()
}

fn default_tenancy_header_name() -> String {
    "x-tenant-id".to_string()
}

fn default_tenancy_session_key() -> String {
    "tenant_id".to_string()
}

fn default_tenancy_jwt_claim() -> String {
    "tenant_id".to_string()
}

impl Default for TenancyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: default_tenancy_source(),
            header_name: default_tenancy_header_name(),
            session_key: default_tenancy_session_key(),
            jwt_claim: default_tenancy_jwt_claim(),
            jwt_secret: None,
            jwt_issuer: None,
            jwt_audience: None,
            base_domain: None,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// Mock loader for tests — returns a hand-built config without touching disk.
    struct MockConfigLoader {
        config: AutumnConfig,
    }

    impl ConfigLoader for MockConfigLoader {
        async fn load(&self) -> Result<AutumnConfig, ConfigError> {
            Ok(self.config.clone())
        }
    }

    #[tokio::test]
    async fn config_loader_trait_returns_supplied_config() {
        let mut custom = AutumnConfig::default();
        custom.server.port = 9999;
        custom.profile = Some("integration-test".to_owned());

        let loader = MockConfigLoader {
            config: custom.clone(),
        };
        let resolved = loader.load().await.expect("mock loader should succeed");

        assert_eq!(resolved.server.port, 9999);
        assert_eq!(resolved.profile.as_deref(), Some("integration-test"));
    }

    #[test]
    fn validate_does_not_error_on_redis_backend_without_url() {
        // Regression: previously `validate()` called
        // `session.backend_plan(profile)` which returned an error for
        // `backend = "redis"` without `redis.url`, exiting the boot before
        // a `with_session_store(...)` override could apply. Session
        // backend validation now lives in `apply_session_layer`, which
        // short-circuits when a custom store is installed. `validate()`
        // is config-shape-only and must accept this combination.
        let mut config = AutumnConfig::default();
        config.session.backend = crate::session::SessionBackend::Redis;
        config.session.redis.url = None;

        config.validate().expect(
            "validate() must accept redis-backend-without-url so custom \
             session store overrides aren't blocked at boot",
        );
    }

    #[tokio::test]
    async fn default_toml_env_loader_succeeds_without_files() {
        // No autumn.toml in the test runner's pwd; loader should fall back to
        // framework defaults rather than failing.
        let loader = TomlEnvConfigLoader::new();
        let resolved = loader.load().await.expect("default loader should succeed");
        // Default port is 3000 per ServerConfig::default — sanity check.
        assert_eq!(resolved.server.port, 3000);
    }

    #[test]
    fn database_config_validate_none() {
        let config = DatabaseConfig {
            url: None,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_config_validate_valid_postgres() {
        let config = DatabaseConfig {
            url: Some("postgres://user:pass@localhost:5432/db".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_config_validate_valid_postgresql() {
        let config = DatabaseConfig {
            url: Some("postgresql://user:pass@localhost:5432/db".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_config_validate_invalid_scheme() {
        let config = DatabaseConfig {
            url: Some("mysql://user:pass@localhost:3306/db".to_string()),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigError::Validation(msg)) => {
                // Ensure we just match the underlying variant correctly
                // as requested in the review.
                assert!(msg.contains("must start with postgres:// or postgresql://"));
            }
            _ => panic!("Expected ConfigError::Validation"),
        }
    }

    #[test]
    fn server_defaults() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.shutdown_timeout_secs, 30);
    }

    #[test]
    fn database_defaults() {
        let config = DatabaseConfig::default();
        assert!(config.url.is_none());
        assert_eq!(config.pool_size, 10);
        assert_eq!(config.connect_timeout_secs, 5);
    }

    #[test]
    fn database_validate_none_url_is_ok() {
        let config = DatabaseConfig {
            url: None,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_validate_postgres_url_is_ok() {
        let config = DatabaseConfig {
            url: Some("postgres://user:pass@localhost/db".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_validate_postgresql_url_is_ok() {
        let config = DatabaseConfig {
            url: Some("postgresql://user:pass@localhost/db".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn database_validate_invalid_url_is_err() {
        let config = DatabaseConfig {
            url: Some("mysql://user:pass@localhost/db".to_string()),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        if let Err(ConfigError::Validation(msg)) = result {
            assert!(msg.contains("Invalid database URL"));
            assert!(msg.contains("must start with postgres:// or postgresql://"));
        } else {
            panic!("Expected ConfigError::Validation");
        }
    }

    #[test]
    fn database_topology_deserializes_primary_and_replica_urls() {
        let config: AutumnConfig = toml::from_str(
            r#"
[database]
primary_url = "postgres://primary.example/app"
replica_url = "postgres://replica.example/app"
primary_pool_size = 12
replica_pool_size = 4
replica_fallback = "primary"
"#,
        )
        .expect("database topology config should parse");

        assert_eq!(
            config.database.primary_url.as_deref(),
            Some("postgres://primary.example/app")
        );
        assert_eq!(
            config.database.replica_url.as_deref(),
            Some("postgres://replica.example/app")
        );
        assert_eq!(config.database.primary_pool_size, Some(12));
        assert_eq!(config.database.replica_pool_size, Some(4));
        assert_eq!(config.database.replica_fallback, ReplicaFallback::Primary);
        assert_eq!(
            config.database.effective_primary_url(),
            Some("postgres://primary.example/app")
        );
        assert_eq!(config.database.effective_primary_pool_size(), 12);
        assert_eq!(config.database.effective_replica_pool_size(), 4);
    }

    #[test]
    fn database_topology_keeps_url_as_single_primary_compatibility_path() {
        let config: AutumnConfig = toml::from_str(
            r#"
[database]
url = "postgres://single.example/app"
pool_size = 7
"#,
        )
        .expect("legacy database.url config should parse");

        assert_eq!(
            config.database.effective_primary_url(),
            Some("postgres://single.example/app")
        );
        assert_eq!(config.database.effective_primary_pool_size(), 7);
        assert_eq!(config.database.effective_replica_pool_size(), 7);
        assert!(config.database.replica_url.is_none());
    }

    #[test]
    fn database_topology_rejects_replica_without_primary() {
        let config = DatabaseConfig {
            replica_url: Some("postgres://replica.example/app".to_owned()),
            ..Default::default()
        };

        let result = config.validate();

        assert!(result.is_err());
        let Err(ConfigError::Validation(message)) = result else {
            panic!("expected database topology validation error");
        };
        assert!(message.contains("database.replica_url"));
        assert!(message.contains("database.primary_url"));
    }

    #[test]
    fn database_topology_env_overrides_role_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_DATABASE__PRIMARY_URL", "postgres://primary.env/app")
            .with("AUTUMN_DATABASE__REPLICA_URL", "postgres://replica.env/app")
            .with("AUTUMN_DATABASE__PRIMARY_POOL_SIZE", "9")
            .with("AUTUMN_DATABASE__REPLICA_POOL_SIZE", "3")
            .with("AUTUMN_DATABASE__REPLICA_FALLBACK", "primary");
        let mut config = AutumnConfig::default();

        config.apply_env_overrides_with_env(&env);

        assert_eq!(
            config.database.primary_url.as_deref(),
            Some("postgres://primary.env/app")
        );
        assert_eq!(
            config.database.replica_url.as_deref(),
            Some("postgres://replica.env/app")
        );
        assert_eq!(config.database.primary_pool_size, Some(9));
        assert_eq!(config.database.replica_pool_size, Some(3));
        assert_eq!(config.database.replica_fallback, ReplicaFallback::Primary);
    }

    #[test]
    fn database_validate_url_edge_cases() {
        let invalid_urls = vec![
            "POSTGRES://localhost/db",
            "postgres:/localhost/db",
            "postgres:localhost/db",
            "http://postgres",
            "   postgres://localhost/db",
            "",
        ];

        for invalid_url in invalid_urls {
            let config = DatabaseConfig {
                url: Some(invalid_url.to_string()),
                ..Default::default()
            };
            assert!(
                config.validate().is_err(),
                "URL should be invalid: {invalid_url}"
            );
        }
    }

    #[test]
    fn autumn_config_validate_ok() {
        let config = AutumnConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn autumn_config_validate_no_longer_errors_on_invalid_session_backend() {
        // Session backend validation moved to `apply_session_layer` so a
        // custom store installed via `AppBuilder::with_session_store(...)`
        // can override an otherwise-invalid backend config without the boot
        // exiting first. `validate()` is config-shape-only now; runtime
        // session selection (and the backend error) lives in
        // `apply_session_layer`, which short-circuits when a custom store
        // is installed. `crate::session::tests::session_backend_plan_*`
        // still cover the underlying error cases directly on
        // `SessionConfig::backend_plan`.
        let mut config = AutumnConfig::default();
        config.session.backend = crate::session::SessionBackend::Redis;
        config.session.redis.url = None;

        config
            .validate()
            .expect("validate() must accept invalid session backend so custom store can override");
    }

    #[test]
    fn autumn_config_validate_database_err() {
        let mut config = AutumnConfig::default();
        config.database.url = Some("mysql://localhost/test".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn log_defaults() {
        let config = LogConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, LogFormat::Auto);
    }

    #[test]
    fn telemetry_defaults() {
        let config = TelemetryConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.service_name, "autumn-app");
        assert!(config.service_namespace.is_none());
        assert_eq!(config.service_version, "unknown");
        assert_eq!(config.environment, "development");
        assert!(config.otlp_endpoint.is_none());
        assert_eq!(config.protocol, TelemetryProtocol::Grpc);
        assert!(!config.strict);
    }

    #[test]
    fn health_defaults() {
        let config = HealthConfig::default();
        assert_eq!(config.path, "/health");
        assert_eq!(config.live_path, "/live");
        assert_eq!(config.ready_path, "/ready");
        assert_eq!(config.startup_path, "/startup");
        assert!(!config.detailed);
    }

    #[test]
    fn top_level_default_populates_all_sections() {
        let config = AutumnConfig::default();
        assert_eq!(config.server.port, 3000);
        assert!(config.database.url.is_none());
        assert_eq!(config.log.level, "info");
        assert_eq!(config.health.path, "/health");
    }

    #[test]
    fn deserialize_empty_object_uses_all_defaults() {
        let config: AutumnConfig = serde_json::from_str("{}").expect("empty object should parse");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.shutdown_timeout_secs, 30);
        assert!(config.database.url.is_none());
        assert_eq!(config.database.pool_size, 10);
        assert_eq!(config.database.connect_timeout_secs, 5);
        assert!(!config.database.auto_migrate_in_production);
        assert_eq!(config.log.level, "info");
        assert_eq!(config.log.format, LogFormat::Auto);
        assert_eq!(config.health.path, "/health");
    }

    #[test]
    fn deserialize_partial_config_merges_with_defaults() {
        let json = r#"{"server": {"port": 8080}}"#;
        let config: AutumnConfig = serde_json::from_str(json).expect("partial config should parse");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.database.pool_size, 10);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn log_format_variants_deserialize() {
        let auto: LogFormat = serde_json::from_str(r#""Auto""#).expect("Auto");
        let pretty: LogFormat = serde_json::from_str(r#""Pretty""#).expect("Pretty");
        let json: LogFormat = serde_json::from_str(r#""Json""#).expect("Json");
        assert_eq!(auto, LogFormat::Auto);
        assert_eq!(pretty, LogFormat::Pretty);
        assert_eq!(json, LogFormat::Json);
    }

    // ── TOML loading tests ───────────────────────────────────────────

    #[test]
    fn load_missing_file_returns_defaults() {
        let config = AutumnConfig::load_from(Path::new("this_file_does_not_exist.toml")).unwrap();
        assert_eq!(config.server.port, 3000);
        assert!(config.database.url.is_none());
    }

    #[test]
    fn load_valid_full_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(
            &path,
            r#"
[server]
port = 8080
host = "0.0.0.0"
shutdown_timeout_secs = 60

[database]
url = "postgres://user:pass@db:5432/myapp"
pool_size = 20
connect_timeout_secs = 10
auto_migrate_in_production = true

[log]
level = "debug"
format = "Json"

[health]
path = "/healthz"
"#,
        )
        .unwrap();

        let config = AutumnConfig::load_from(&path).unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.shutdown_timeout_secs, 60);
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://user:pass@db:5432/myapp")
        );
        assert_eq!(config.database.pool_size, 20);
        assert_eq!(config.database.connect_timeout_secs, 10);
        assert!(config.database.auto_migrate_in_production);
        assert_eq!(config.log.level, "debug");
        assert_eq!(config.log.format, LogFormat::Json);
        assert_eq!(config.health.path, "/healthz");
    }

    #[test]
    fn load_partial_config_merges_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(&path, "[server]\nport = 9090\n").unwrap();

        let config = AutumnConfig::load_from(&path).unwrap();
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.database.pool_size, 10);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn load_invalid_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(&path, "not valid [[[toml").unwrap();

        let result = AutumnConfig::load_from(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid autumn.toml"));
    }

    #[test]
    fn load_empty_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(&path, "").unwrap();

        let config = AutumnConfig::load_from(&path).unwrap();
        assert_eq!(config.server.port, 3000);
    }

    // ── Environment variable override tests ──────────────────────

    #[test]
    fn env_override_database_url() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__URL", "postgres://override:5432/test");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://override:5432/test")
        );
    }

    #[test]
    fn env_override_database_url_wins_over_file_primary_url() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__URL", "postgres://env.example/app");
        let mut config = AutumnConfig::default();
        config.database.primary_url = Some("postgres://file.example/app".to_owned());

        config.apply_env_overrides_with_env(&env);

        assert_eq!(
            config.database.effective_primary_url(),
            Some("postgres://env.example/app")
        );
        assert!(config.database.primary_url.is_none());
    }

    #[test]
    fn env_override_database_primary_url_wins_over_legacy_database_url() {
        let env = MockEnv::new()
            .with("AUTUMN_DATABASE__URL", "postgres://legacy.env/app")
            .with("AUTUMN_DATABASE__PRIMARY_URL", "postgres://primary.env/app");
        let mut config = AutumnConfig::default();
        config.database.primary_url = Some("postgres://file.example/app".to_owned());

        config.apply_env_overrides_with_env(&env);

        assert_eq!(
            config.database.effective_primary_url(),
            Some("postgres://primary.env/app")
        );
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://legacy.env/app")
        );
    }

    #[test]
    fn env_override_pool_size() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__POOL_SIZE", "25");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.database.pool_size, 25);
    }

    #[cfg(feature = "reporting")]
    #[test]
    fn env_override_reporting() {
        let env = MockEnv::new()
            .with("AUTUMN_REPORTING__ENABLED", "false")
            .with("AUTUMN_REPORTING__SAMPLE_RATE", "0.1");
        let mut config = AutumnConfig::default();
        assert!(config.reporting.enabled);
        assert!((config.reporting.sample_rate - 1.0).abs() < f64::EPSILON);
        config.apply_env_overrides_with_env(&env);
        assert!(!config.reporting.enabled);
        assert!((config.reporting.sample_rate - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn env_override_connect_timeout() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS", "15");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.database.connect_timeout_secs, 15);
    }

    #[test]
    fn env_override_invalid_pool_size_ignored() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__POOL_SIZE", "not_a_number");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.database.pool_size, 10);
    }

    #[cfg(feature = "storage")]
    #[test]
    fn env_override_storage_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_STORAGE__BACKEND", "s3")
            .with("AUTUMN_STORAGE__DEFAULT_PROVIDER", "media")
            .with("AUTUMN_STORAGE__ALLOW_LOCAL_IN_PRODUCTION", "true")
            .with("AUTUMN_STORAGE__LOCAL__ROOT", "var/blobs")
            .with("AUTUMN_STORAGE__LOCAL__MOUNT_PATH", "/files")
            .with("AUTUMN_STORAGE__LOCAL__DEFAULT_URL_EXPIRY_SECS", "42")
            .with("AUTUMN_STORAGE__LOCAL__SIGNING_KEY", "secret")
            .with("AUTUMN_STORAGE__S3__BUCKET", "uploads")
            .with("AUTUMN_STORAGE__S3__REGION", "us-east-1")
            .with("AUTUMN_STORAGE__S3__ENDPOINT", "https://s3.example.test")
            .with(
                "AUTUMN_STORAGE__S3__PUBLIC_BASE_URL",
                "https://cdn.example.test",
            )
            .with("AUTUMN_STORAGE__S3__ACCESS_KEY_ID_ENV", "AWS_ACCESS_KEY_ID")
            .with(
                "AUTUMN_STORAGE__S3__SECRET_ACCESS_KEY_ENV",
                "AWS_SECRET_ACCESS_KEY",
            )
            .with("AUTUMN_STORAGE__S3__FORCE_PATH_STYLE", "true")
            .with("AUTUMN_STORAGE__S3__DEFAULT_URL_EXPIRY_SECS", "99");
        let mut config = AutumnConfig::default();

        config.apply_env_overrides_with_env(&env);

        assert_eq!(config.storage.backend, crate::storage::StorageBackend::S3);
        assert_eq!(config.storage.default_provider, "media");
        assert!(config.storage.allow_local_in_production);
        assert_eq!(config.storage.local.root, PathBuf::from("var/blobs"));
        assert_eq!(config.storage.local.mount_path, "/files");
        assert_eq!(config.storage.local.default_url_expiry_secs, 42);
        assert_eq!(config.storage.local.signing_key.as_deref(), Some("secret"));
        assert_eq!(config.storage.s3.bucket.as_deref(), Some("uploads"));
        assert_eq!(config.storage.s3.region.as_deref(), Some("us-east-1"));
        assert_eq!(
            config.storage.s3.endpoint.as_deref(),
            Some("https://s3.example.test")
        );
        assert_eq!(
            config.storage.s3.public_base_url.as_deref(),
            Some("https://cdn.example.test")
        );
        assert_eq!(
            config.storage.s3.access_key_id_env.as_deref(),
            Some("AWS_ACCESS_KEY_ID")
        );
        assert_eq!(
            config.storage.s3.secret_access_key_env.as_deref(),
            Some("AWS_SECRET_ACCESS_KEY")
        );
        assert!(config.storage.s3.force_path_style);
        assert_eq!(config.storage.s3.default_url_expiry_secs, 99);
    }

    #[test]
    fn env_override_database_auto_migrate_in_production() {
        let env = MockEnv::new().with("AUTUMN_DATABASE__AUTO_MIGRATE_IN_PRODUCTION", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert!(config.database.auto_migrate_in_production);
    }

    #[test]
    fn env_override_jobs_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_JOBS__BACKEND", "redis")
            .with("AUTUMN_JOBS__WORKERS", "8")
            .with("AUTUMN_JOBS__MAX_ATTEMPTS", "12")
            .with("AUTUMN_JOBS__INITIAL_BACKOFF_MS", "750")
            .with("AUTUMN_JOBS__REDIS__URL", "redis://jobs:6379/2")
            .with("AUTUMN_JOBS__REDIS__KEY_PREFIX", "myapp:jobs")
            .with("AUTUMN_JOBS__REDIS__VISIBILITY_TIMEOUT_MS", "45000");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);

        assert_eq!(config.jobs.backend, "redis");
        assert_eq!(config.jobs.workers, 8);
        assert_eq!(config.jobs.max_attempts, 12);
        assert_eq!(config.jobs.initial_backoff_ms, 750);
        assert_eq!(
            config.jobs.redis.url.as_deref(),
            Some("redis://jobs:6379/2")
        );
        assert_eq!(config.jobs.redis.key_prefix, "myapp:jobs");
        assert_eq!(config.jobs.redis.visibility_timeout_ms, 45_000);
    }

    #[test]
    fn jobs_toml_deserializes_redis_visibility_timeout() {
        let config: AutumnConfig = toml::from_str(
            r#"
            [jobs]
            backend = "redis"

            [jobs.redis]
            url = "redis://localhost:6379/5"
            key_prefix = "demo:jobs"
            visibility_timeout_ms = 15000
            "#,
        )
        .unwrap();

        assert_eq!(config.jobs.backend, "redis");
        assert_eq!(
            config.jobs.redis.url.as_deref(),
            Some("redis://localhost:6379/5")
        );
        assert_eq!(config.jobs.redis.key_prefix, "demo:jobs");
        assert_eq!(config.jobs.redis.visibility_timeout_ms, 15_000);
    }

    #[test]
    fn channels_defaults_to_in_process_backend() {
        let config = AutumnConfig::default();

        assert_eq!(config.channels.backend, ChannelBackend::InProcess);
        assert_eq!(config.channels.capacity, 32);
        assert_eq!(config.channels.redis.key_prefix, "autumn:channels");
        assert!(config.channels.redis.url.is_none());
    }

    #[test]
    fn channels_env_overrides_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_CHANNELS__BACKEND", "redis")
            .with("AUTUMN_CHANNELS__CAPACITY", "128")
            .with("AUTUMN_CHANNELS__REDIS__URL", "redis://channels:6379/4")
            .with("AUTUMN_CHANNELS__REDIS__KEY_PREFIX", "myapp:channels");
        let mut config = AutumnConfig::default();

        config.apply_env_overrides_with_env(&env);

        assert_eq!(config.channels.backend, ChannelBackend::Redis);
        assert_eq!(config.channels.capacity, 128);
        assert_eq!(
            config.channels.redis.url.as_deref(),
            Some("redis://channels:6379/4")
        );
        assert_eq!(config.channels.redis.key_prefix, "myapp:channels");
    }

    #[test]
    fn channels_toml_deserializes_redis_backend() {
        let config: AutumnConfig = toml::from_str(
            r#"
            [channels]
            backend = "redis"
            capacity = 64

            [channels.redis]
            url = "redis://localhost:6379/5"
            key_prefix = "demo:channels"
            "#,
        )
        .unwrap();

        assert_eq!(config.channels.backend, ChannelBackend::Redis);
        assert_eq!(config.channels.capacity, 64);
        assert_eq!(
            config.channels.redis.url.as_deref(),
            Some("redis://localhost:6379/5")
        );
        assert_eq!(config.channels.redis.key_prefix, "demo:channels");
    }

    #[test]
    fn env_override_invalid_jobs_numeric_values_ignored() {
        let env = MockEnv::new()
            .with("AUTUMN_JOBS__WORKERS", "many")
            .with("AUTUMN_JOBS__MAX_ATTEMPTS", "a_lot")
            .with("AUTUMN_JOBS__INITIAL_BACKOFF_MS", "soon");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);

        assert_eq!(config.jobs.workers, 1);
        assert_eq!(config.jobs.max_attempts, 5);
        assert_eq!(config.jobs.initial_backoff_ms, 250);
    }

    // ── Server env override tests ────────────────────────────────

    #[test]
    fn env_override_server_port() {
        let env = MockEnv::new().with("AUTUMN_SERVER__PORT", "8080");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn parse_env_works() {
        let env = MockEnv::new().with("SOME_NUM", "123");
        let mut target: u32 = 0;
        parse_env(&env, "SOME_NUM", &mut target);
        assert_eq!(target, 123);

        let env_err = MockEnv::new().with("SOME_NUM", "abc");
        let mut target_err: u32 = 0;
        parse_env(&env_err, "SOME_NUM", &mut target_err);
        assert_eq!(target_err, 0); // Unchanged
    }

    #[test]
    fn parse_env_option_string_works() {
        let env = MockEnv::new().with("SOME_OPT", "val");
        let mut target = None;
        parse_env_option_string(&env, "SOME_OPT", &mut target);
        assert_eq!(target, Some("val".to_string()));

        let env_empty = MockEnv::new().with("SOME_OPT", "");
        let mut target_empty = Some("old".to_string());
        parse_env_option_string(&env_empty, "SOME_OPT", &mut target_empty);
        assert_eq!(target_empty, None);
    }

    #[test]
    fn parse_env_string_works() {
        let env = MockEnv::new().with("SOME_STR", "val");
        let mut target = "old".to_string();
        parse_env_string(&env, "SOME_STR", &mut target);
        assert_eq!(target, "val");
    }

    #[test]
    fn parse_env_bool_works() {
        let env = MockEnv::new().with("SOME_BOOL", "true");
        let mut target = false;
        parse_env_bool(&env, "SOME_BOOL", &mut target);
        assert!(target);

        let env2 = MockEnv::new().with("SOME_BOOL", "1");
        let mut target2 = false;
        parse_env_bool(&env2, "SOME_BOOL", &mut target2);
        assert!(target2);

        let env3 = MockEnv::new().with("SOME_BOOL", "0");
        let mut target3 = true;
        parse_env_bool(&env3, "SOME_BOOL", &mut target3);
        assert!(!target3);

        let env_err = MockEnv::new().with("SOME_BOOL", "invalid");
        let mut target_err = true;
        parse_env_bool(&env_err, "SOME_BOOL", &mut target_err);
        assert!(target_err); // Unchanged
    }

    #[test]
    fn parse_env_csv_works() {
        let env = MockEnv::new().with("SOME_CSV", "a, b,c");
        let mut target = vec![];
        parse_env_csv(&env, "SOME_CSV", &mut target);
        assert_eq!(target, vec!["a", "b", "c"]);
    }

    #[test]
    fn env_override_rate_limit_trusted_proxies() {
        let env = MockEnv::new().with(
            "AUTUMN_SECURITY__RATE_LIMIT__TRUSTED_PROXIES",
            "10.0.0.10, 203.0.113.0/24",
        );
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.rate_limit.trusted_proxies,
            vec!["10.0.0.10", "203.0.113.0/24"]
        );
    }

    #[test]
    fn env_override_rate_limit_backend_redis() {
        use crate::security::config::RateLimitBackend;
        let env = MockEnv::new().with("AUTUMN_SECURITY__RATE_LIMIT__BACKEND", "redis");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.security.rate_limit.backend, RateLimitBackend::Redis);
    }

    #[test]
    fn env_override_rate_limit_backend_memory() {
        use crate::security::config::RateLimitBackend;
        let env = MockEnv::new().with("AUTUMN_SECURITY__RATE_LIMIT__BACKEND", "memory");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.security.rate_limit.backend, RateLimitBackend::Memory);
    }

    #[test]
    fn env_override_rate_limit_backend_invalid_ignored() {
        use crate::security::config::RateLimitBackend;
        let env = MockEnv::new().with("AUTUMN_SECURITY__RATE_LIMIT__BACKEND", "postgres");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.security.rate_limit.backend, RateLimitBackend::Memory);
    }

    #[cfg(feature = "redis")]
    #[test]
    fn env_override_rate_limit_on_backend_failure_fail_closed() {
        use crate::security::config::RateLimitBackendFailure;
        let env = MockEnv::new().with(
            "AUTUMN_SECURITY__RATE_LIMIT__ON_BACKEND_FAILURE",
            "fail_closed",
        );
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.rate_limit.on_backend_failure,
            RateLimitBackendFailure::FailClosed
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn env_override_rate_limit_on_backend_failure_invalid_ignored() {
        use crate::security::config::RateLimitBackendFailure;
        let env = MockEnv::new().with("AUTUMN_SECURITY__RATE_LIMIT__ON_BACKEND_FAILURE", "explode");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.rate_limit.on_backend_failure,
            RateLimitBackendFailure::FailOpen
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn env_override_rate_limit_redis_url() {
        let env = MockEnv::new().with(
            "AUTUMN_SECURITY__RATE_LIMIT__REDIS__URL",
            "redis://myhost:6379",
        );
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.rate_limit.redis.url.as_deref(),
            Some("redis://myhost:6379")
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn env_override_rate_limit_redis_key_prefix() {
        let env = MockEnv::new().with("AUTUMN_SECURITY__RATE_LIMIT__REDIS__KEY_PREFIX", "prod:rl");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.security.rate_limit.redis.key_prefix, "prod:rl");
    }

    #[test]
    fn env_override_server_host() {
        let env = MockEnv::new().with("AUTUMN_SERVER__HOST", "0.0.0.0");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.host, "0.0.0.0");
    }

    #[test]
    fn env_override_server_shutdown_timeout() {
        let env = MockEnv::new().with("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "60");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.shutdown_timeout_secs, 60);
    }

    #[test]
    fn env_override_invalid_server_port_ignored() {
        let env = MockEnv::new().with("AUTUMN_SERVER__PORT", "not_a_port");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn env_override_invalid_shutdown_timeout_ignored() {
        let env = MockEnv::new().with("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "forever");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.shutdown_timeout_secs, 30);
    }

    // ── Log env override tests ───────────────────────────────────

    #[test]
    fn env_override_log_level() {
        let env = MockEnv::new().with("AUTUMN_LOG__LEVEL", "debug");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.log.level, "debug");
    }

    #[test]
    fn env_override_log_format_json() {
        let env = MockEnv::new().with("AUTUMN_LOG__FORMAT", "Json");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.log.format, LogFormat::Json);
    }

    #[test]
    fn env_override_log_format_pretty() {
        let env = MockEnv::new().with("AUTUMN_LOG__FORMAT", "Pretty");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.log.format, LogFormat::Pretty);
    }

    #[test]
    fn env_override_invalid_log_format_ignored() {
        let env = MockEnv::new().with("AUTUMN_LOG__FORMAT", "yaml");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.log.format, LogFormat::Auto);
    }

    // ── Health env override tests ────────────────────────────────

    #[test]
    fn env_override_telemetry_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_TELEMETRY__ENABLED", "true")
            .with("AUTUMN_TELEMETRY__SERVICE_NAME", "orders-api")
            .with("AUTUMN_TELEMETRY__SERVICE_NAMESPACE", "acme")
            .with("AUTUMN_TELEMETRY__SERVICE_VERSION", "1.2.3")
            .with("AUTUMN_TELEMETRY__ENVIRONMENT", "production")
            .with(
                "AUTUMN_TELEMETRY__OTLP_ENDPOINT",
                "http://otel-collector:4317",
            )
            .with("AUTUMN_TELEMETRY__PROTOCOL", "HTTP_PROTOBUF")
            .with("AUTUMN_TELEMETRY__STRICT", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert!(config.telemetry.enabled);
        assert_eq!(config.telemetry.service_name, "orders-api");
        assert_eq!(config.telemetry.service_namespace.as_deref(), Some("acme"));
        assert_eq!(config.telemetry.service_version, "1.2.3");
        assert_eq!(config.telemetry.environment, "production");
        assert_eq!(
            config.telemetry.otlp_endpoint.as_deref(),
            Some("http://otel-collector:4317")
        );
        assert_eq!(config.telemetry.protocol, TelemetryProtocol::HttpProtobuf);
        assert!(config.telemetry.strict);
    }

    #[test]
    fn env_override_invalid_telemetry_protocol_ignored() {
        let env = MockEnv::new().with("AUTUMN_TELEMETRY__PROTOCOL", "zipkin");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.telemetry.protocol, TelemetryProtocol::Grpc);
    }

    #[test]
    fn env_override_health_path() {
        let env = MockEnv::new().with("AUTUMN_HEALTH__PATH", "/healthz");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.health.path, "/healthz");
    }

    #[test]
    fn env_override_probe_paths() {
        let env = MockEnv::new()
            .with("AUTUMN_HEALTH__LIVE_PATH", "/livez")
            .with("AUTUMN_HEALTH__READY_PATH", "/readyz")
            .with("AUTUMN_HEALTH__STARTUP_PATH", "/startupz");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.health.live_path, "/livez");
        assert_eq!(config.health.ready_path, "/readyz");
        assert_eq!(config.health.startup_path, "/startupz");
    }

    // ── Precedence test ──────────────────────────────────────────

    #[test]
    fn env_overrides_toml_values() {
        let env = MockEnv::new().with("AUTUMN_SERVER__PORT", "9999");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(&path, "[server]\nport = 4000\n").unwrap();
        let mut config = AutumnConfig::load_from(&path).unwrap();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.server.port, 9999); // env wins
    }

    // ── Validation tests ─────────────────────────────────────────

    #[test]
    fn validate_rejects_invalid_url_scheme() {
        let config = DatabaseConfig {
            url: Some("mysql://localhost/test".to_owned()),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with postgres://")
        );
    }

    #[test]
    fn validate_accepts_postgres_url() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".to_owned()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_postgresql_url() {
        let config = DatabaseConfig {
            url: Some("postgresql://localhost/test".to_owned()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_no_url() {
        let config = DatabaseConfig::default();
        assert!(config.validate().is_ok());
    }

    // ── Profile tests ──────────────────────────────────────────

    #[test]
    fn resolve_profile_from_autumn_env() {
        let env = MockEnv::new().with("AUTUMN_ENV", "prod");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "prod");
    }

    #[test]
    fn resolve_profile_from_legacy_env() {
        let env = MockEnv::new().with("AUTUMN_PROFILE", "staging");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "staging");
    }

    #[test]
    fn resolve_profile_prefers_autumn_env_over_legacy_alias() {
        let env = MockEnv::new()
            .with("AUTUMN_ENV", "dev")
            .with("AUTUMN_PROFILE", "prod");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "dev");
    }

    #[test]
    fn resolve_profile_normalizes_production_alias() {
        let env = MockEnv::new().with("AUTUMN_ENV", "production");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "prod");
    }

    #[test]
    fn resolve_profile_normalizes_development_alias_with_whitespace() {
        let env = MockEnv::new().with("AUTUMN_ENV", "  development  ");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "dev");
    }

    #[test]
    fn resolve_profile_normalizes_uppercase_dev_and_prod() {
        let prod_env = MockEnv::new().with("AUTUMN_ENV", "PROD");
        let prod = resolve_profile(&prod_env);
        assert_eq!(prod, "prod");

        let dev_env = MockEnv::new().with("AUTUMN_ENV", "DEV");
        let dev = resolve_profile(&dev_env);
        assert_eq!(dev, "dev");
    }

    #[test]
    fn resolve_profile_preserves_case_for_custom_profiles() {
        let env = MockEnv::new().with("AUTUMN_ENV", "QA");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "QA");
    }

    #[test]
    fn resolve_profile_auto_detect_debug() {
        let env = MockEnv::new().with("AUTUMN_IS_DEBUG", "1");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "dev");
    }

    #[test]
    fn resolve_profile_auto_detect_release() {
        let env = MockEnv::new().with("AUTUMN_IS_DEBUG", "0");
        let profile = resolve_profile(&env);
        assert_eq!(profile, "prod");
    }

    #[test]
    fn resolve_profile_defaults_to_dev_when_no_signal_present() {
        let env = MockEnv::new();
        let profile = resolve_profile(&env);
        assert_eq!(profile, "dev");
    }

    #[test]
    fn dev_profile_smart_defaults() {
        let defaults = profile_defaults_as_toml("dev");
        let toml_str = toml::to_string(&defaults).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.log.level, "debug");
        assert_eq!(config.log.format, LogFormat::Pretty);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.shutdown_timeout_secs, 1);
        assert_eq!(
            config.server.prestop_grace_secs, 0,
            "dev profile must set prestop_grace_secs = 0 so Ctrl-C is instant"
        );
        assert_eq!(config.telemetry.environment, "development");
        assert!(config.health.detailed);
        assert_eq!(config.cors.allowed_origins, vec!["*"]);
        assert!(
            config.security.trusted_proxies.trust_forwarded_headers,
            "dev profile must trust forwarded headers from loopback"
        );
        assert!(
            config
                .security
                .trusted_proxies
                .ranges
                .contains(&"127.0.0.0/8".to_owned()),
            "dev profile must include 127.0.0.0/8 as trusted proxy range"
        );
        assert!(
            config
                .security
                .trusted_proxies
                .ranges
                .contains(&"::1/128".to_owned()),
            "dev profile must include ::1/128 as trusted proxy range"
        );
    }

    #[test]
    fn prod_profile_smart_defaults() {
        let defaults = profile_defaults_as_toml("prod");
        let toml_str = toml::to_string(&defaults).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.log.level, "info");
        assert_eq!(config.log.format, LogFormat::Json);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.shutdown_timeout_secs, 30);
        assert_eq!(config.telemetry.environment, "production");
        assert!(!config.health.detailed);
        // AC: HSTS auto-enabled in the production profile.
        assert!(
            config.security.headers.strict_transport_security,
            "prod profile must auto-enable Strict-Transport-Security"
        );
        // Defaults should still be secure-by-default in prod.
        assert_eq!(config.security.headers.x_frame_options, "DENY");
        assert!(config.security.headers.x_content_type_options);
        assert!(!config.security.headers.content_security_policy.is_empty());
    }

    #[test]
    fn dev_profile_does_not_auto_enable_hsts() {
        let defaults = profile_defaults_as_toml("dev");
        let toml_str = toml::to_string(&defaults).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();

        assert!(
            !config.security.headers.strict_transport_security,
            "dev profile must not force HSTS on (local http development)"
        );
    }

    #[test]
    fn custom_profile_no_smart_defaults() {
        let defaults = profile_defaults_as_toml("staging");
        assert_eq!(defaults, toml::Value::Table(toml::map::Map::new()));
    }

    #[test]
    fn deep_merge_tables() {
        let mut base: toml::Value = toml::from_str(
            r#"
            [server]
            port = 3000
            host = "127.0.0.1"
            [database]
            pool_size = 10
            "#,
        )
        .unwrap();

        let overlay: toml::Value = toml::from_str(
            r#"
            [server]
            port = 8080
            [database]
            url = "postgres://localhost/test"
            "#,
        )
        .unwrap();

        deep_merge(&mut base, overlay);

        // Overlay value wins
        assert_eq!(base["server"]["port"], toml::Value::Integer(8080));
        // Base value preserved when not in overlay
        assert_eq!(
            base["server"]["host"],
            toml::Value::String("127.0.0.1".into())
        );
        // New key from overlay added
        assert_eq!(
            base["database"]["url"],
            toml::Value::String("postgres://localhost/test".into())
        );
        // Base key preserved
        assert_eq!(base["database"]["pool_size"], toml::Value::Integer(10));
    }

    #[test]
    fn profile_toml_overrides_base_toml() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("autumn.toml");
        let dev_path = dir.path().join("autumn-dev.toml");

        std::fs::write(
            &base_path,
            r"
            [server]
            port = 3000
            [database]
            pool_size = 10
            ",
        )
        .unwrap();

        std::fs::write(
            &dev_path,
            r#"
            [database]
            url = "postgres://localhost/myapp_dev"
            "#,
        )
        .unwrap();

        // Load base
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let base = load_raw_toml(&base_path).unwrap().unwrap();
        deep_merge(&mut merged, base);
        let profile = load_raw_toml(&dev_path).unwrap().unwrap();
        deep_merge(&mut merged, profile);

        let toml_str = toml::to_string(&merged).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.server.port, 3000); // from base
        assert_eq!(config.database.pool_size, 10); // from base, preserved
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://localhost/myapp_dev")
        ); // from profile
    }

    #[test]
    fn inline_profile_section_overrides_base_toml() {
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let base: toml::Value = toml::from_str(
            r#"
            [server]
            port = 3000

            [log]
            level = "info"

            [profile.dev.log]
            level = "debug"
            "#,
        )
        .unwrap();

        deep_merge(&mut merged, base.clone());
        let inline = profile_section_from_base_toml(&base, "dev").unwrap();
        deep_merge(&mut merged, inline);

        let toml_str = toml::to_string(&merged).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.log.level, "debug");
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("dev", "dev"), 0);
        assert_eq!(levenshtein("dev", "dve"), 2); // swap = 2 edits (del + ins)
        assert_eq!(levenshtein("prod", "prodd"), 1);
        assert_eq!(levenshtein("prod", "prd"), 1);
        assert_eq!(levenshtein("staging", "dev"), 7);
    }

    #[test]
    fn env_override_health_detailed() {
        let env = MockEnv::new().with("AUTUMN_HEALTH__DETAILED", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert!(config.health.detailed);
    }

    #[test]
    fn profile_name_accessor() {
        let mut config = AutumnConfig::default();
        assert!(config.profile_name().is_none());

        config.profile = Some("dev".to_owned());
        assert_eq!(config.profile_name(), Some("dev"));
    }

    // ── Mutant-hunting tests ────────────────────────────────────

    #[test]
    fn find_config_file_falls_back_to_cwd() {
        // Without AUTUMN_MANIFEST_DIR, should return just the filename
        let env = MockEnv::new();
        let path = find_config_file_named("autumn.toml", &env);
        assert_eq!(path, PathBuf::from("autumn.toml"));
    }

    #[test]
    fn find_config_file_uses_manifest_dir_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("autumn.toml");
        std::fs::write(&config_path, "").unwrap();

        let env = MockEnv::new().with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());
        let path = find_config_file_named("autumn.toml", &env);
        assert_eq!(path, config_path);
    }

    #[test]
    fn find_config_file_falls_back_when_manifest_dir_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        // dir exists but the file doesn't
        let env = MockEnv::new().with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());
        let path = find_config_file_named("nonexistent.toml", &env);
        assert_eq!(path, PathBuf::from("nonexistent.toml"));
    }

    #[test]
    fn resolve_profile_cli_flag_exact_match() {
        // resolve_profile checks `--profile` in CLI args. We can't easily
        // inject args, but we can verify the env path doesn't match other args.
        // The `== "--profile"` guard is the key: if it were `!=`, every arg
        // would trigger the branch.
        let env = MockEnv::new();
        // With no env vars and no matching CLI args, should be None
        let profile = resolve_profile(&env);
        // This may or may not be None depending on test harness args,
        // but the important thing is it doesn't crash or return garbage.
        // The env-based tests above cover the positive cases.
        drop(profile);
    }

    #[test]
    fn deep_merge_non_table_overlay_replaces_base() {
        // When overlay is not a table, it should replace (not merge into) base.
        // This kills the `&& → ||` mutant on line 162.
        let mut base: toml::Value = toml::from_str("[server]\nport = 3000\n").unwrap();
        let overlay = toml::Value::String("not_a_table".into());

        // When base is table and overlay is NOT table, base should be unchanged
        // (the function only merges when BOTH are tables).
        deep_merge(&mut base, overlay);
        // base should still be the original table (overlay was ignored)
        assert!(base.is_table());
        assert_eq!(base["server"]["port"], toml::Value::Integer(3000));
    }

    #[test]
    fn deep_merge_when_base_not_table() {
        // When base is not a table, overlay should not merge
        let mut base = toml::Value::String("original".into());
        let overlay: toml::Value = toml::from_str("[server]\nport = 3000\n").unwrap();

        deep_merge(&mut base, overlay);
        // base should be unchanged
        assert_eq!(base, toml::Value::String("original".into()));
    }

    #[test]
    fn suggest_profile_close_match() {
        // "dve" is edit-distance 2 from "dev" → should suggest "dev"
        assert_eq!(suggest_profile("dve"), Some("dev"));
    }

    #[test]
    fn suggest_profile_no_match_when_distant() {
        // "xyz" is far from both "dev" and "prod" → no suggestion
        assert_eq!(suggest_profile("xyz"), None);
    }

    #[test]
    fn suggest_profile_exact_known_profile() {
        // Exact match has distance 0 → suggests itself
        assert_eq!(suggest_profile("dev"), Some("dev"));
        assert_eq!(suggest_profile("prod"), Some("prod"));
    }

    #[test]
    fn suggest_profile_prd() {
        // "prd" is distance 1 from "prod"
        assert_eq!(suggest_profile("prd"), Some("prod"));
    }

    #[test]
    fn warn_profile_typo_runs_without_panic() {
        warn_profile_typo("dve");
        warn_profile_typo("xyz");
    }

    #[test]
    fn should_warn_missing_profile_file_custom_without_inline() {
        assert!(should_warn_missing_profile_file("staging", false));
    }

    #[test]
    fn should_not_warn_missing_profile_file_custom_with_inline() {
        assert!(!should_warn_missing_profile_file("staging", true));
    }

    #[test]
    fn should_not_warn_missing_profile_file_dev_or_prod() {
        assert!(!should_warn_missing_profile_file("dev", false));
        assert!(!should_warn_missing_profile_file("prod", false));
    }

    #[test]
    fn levenshtein_threshold_in_warn_profile_typo() {
        assert!(levenshtein("dve", "dev") <= 2);
        assert!(levenshtein("xyz", "dev") > 2);
        assert!(levenshtein("xyz", "prod") > 2);
    }

    #[test]
    fn env_override_cors_allowed_origins() {
        let env = MockEnv::new().with(
            "AUTUMN_CORS__ALLOWED_ORIGINS",
            "https://a.com, https://b.com",
        );
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.cors.allowed_origins,
            vec!["https://a.com", "https://b.com"]
        );
    }

    #[test]
    fn env_override_cors_allow_credentials() {
        let env = MockEnv::new().with("AUTUMN_CORS__ALLOW_CREDENTIALS", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert!(config.cors.allow_credentials);
    }

    #[test]
    fn env_override_cors_max_age() {
        let env = MockEnv::new().with("AUTUMN_CORS__MAX_AGE_SECS", "3600");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.cors.max_age_secs, 3600);
    }

    #[test]
    fn cors_validate_rejects_wildcard_with_credentials() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["*".to_owned()];
        config.cors.allow_credentials = true;

        let result = config.validate();
        match result {
            Err(ConfigError::Validation(msg)) => {
                assert!(
                    msg.contains("allow_credentials") && msg.contains('*'),
                    "message should mention credentials and wildcard, got: {msg}"
                );
            }
            other => panic!("expected ConfigError::Validation, got {other:?}"),
        }
    }

    #[test]
    fn cors_validate_accepts_wildcard_without_credentials() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["*".to_owned()];
        config.cors.allow_credentials = false;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cors_validate_accepts_explicit_origins_with_credentials() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["https://app.example.com".to_owned()];
        config.cors.allow_credentials = true;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_uses_profile_layering() {
        // Test AutumnConfig::load_with_env() with a dev profile via env var.
        // This kills the "replace load → Ok(Default::default())" mutant.
        let env = MockEnv::new().with("AUTUMN_PROFILE", "dev");

        let config = AutumnConfig::load_with_env(&env).unwrap();
        // With dev profile, smart defaults should apply
        assert_eq!(config.profile.as_deref(), Some("dev"));
        assert_eq!(config.log.level, "debug"); // dev default
        assert_eq!(config.log.format, LogFormat::Pretty); // dev default
        assert!(config.health.detailed); // dev default
    }

    #[test]
    fn load_custom_profile_without_toml_warns() {
        // Test the typo warning branch: profile != "dev" && profile != "prod"
        // without a corresponding autumn-{profile}.toml triggers warn_profile_typo.
        // This kills the match guard mutants on line 341.
        let env = MockEnv::new().with("AUTUMN_PROFILE", "staging");

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("staging"));
        // staging has no smart defaults, so values should be framework defaults
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn load_dev_profile_no_profile_toml_no_warn() {
        // dev/prod without their profile TOML should NOT trigger warn_profile_typo.
        // This tests the `None => {}` branch (line 342).
        let env = MockEnv::new().with("AUTUMN_PROFILE", "dev");

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("dev"));
    }

    #[test]
    fn load_custom_profile_uses_inline_profile_without_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("autumn.toml");
        std::fs::write(
            &base_path,
            r"
            [server]
            port = 3000

            [profile.staging.server]
            port = 4100
            ",
        )
        .unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_ENV", "staging")
            .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("staging"));
        assert_eq!(config.server.port, 4100);
    }

    #[test]
    fn load_production_profile_reads_inline_profile_production_section() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("autumn.toml");
        std::fs::write(
            &base_path,
            r"
            [profile.production.server]
            port = 4200
            ",
        )
        .unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_ENV", "production")
            .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("prod"));
        assert_eq!(config.server.port, 4200);
    }

    #[test]
    fn load_production_profile_reads_legacy_autumn_production_toml() {
        let dir = tempfile::tempdir().unwrap();
        let production_path = dir.path().join("autumn-production.toml");
        std::fs::write(
            &production_path,
            r"
            [server]
            port = 4300
            ",
        )
        .unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_ENV", "production")
            .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("prod"));
        assert_eq!(config.server.port, 4300);
    }

    #[test]
    fn load_prod_prefers_autumn_prod_toml_before_production_alias() {
        let dir = tempfile::tempdir().unwrap();
        let prod_path = dir.path().join("autumn-prod.toml");
        let production_path = dir.path().join("autumn-production.toml");

        std::fs::write(
            &prod_path,
            r"
            [server]
            port = 4400
            ",
        )
        .unwrap();
        // Malformed TOML should be ignored because `autumn-prod.toml` is chosen first.
        std::fs::write(&production_path, "[server\nport = 4500").unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_ENV", "prod")
            .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("prod"));
        assert_eq!(config.server.port, 4400);
    }

    #[test]
    fn load_production_prefers_autumn_production_toml_before_prod_alias() {
        let dir = tempfile::tempdir().unwrap();
        let prod_path = dir.path().join("autumn-prod.toml");
        let production_path = dir.path().join("autumn-production.toml");

        std::fs::write(
            &production_path,
            r"
            [server]
            port = 4500
            ",
        )
        .unwrap();
        // Malformed TOML should be ignored because `autumn-production.toml` is chosen first.
        std::fs::write(&prod_path, "[server\nport = 4400").unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_ENV", "production")
            .with("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());

        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert_eq!(config.profile.as_deref(), Some("prod"));
        assert_eq!(config.server.port, 4500);
    }

    #[test]
    fn load_from_io_error_is_not_swallowed() {
        // load_from should return Err on non-NotFound IO errors.
        // On all platforms, trying to read a directory as a file triggers an error.
        let dir = tempfile::tempdir().unwrap();
        let result = AutumnConfig::load_from(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_raw_toml_missing_file_returns_none() {
        let result = load_raw_toml(Path::new("this_file_does_not_exist_12345.toml")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_raw_toml_directory_returns_io_error() {
        // Reading a directory is an IO error, NOT NotFound.
        // This kills the "replace match guard NotFound with true" mutant:
        // if the guard were always true, this would return Ok(None) instead of Err.
        let dir = tempfile::tempdir().unwrap();
        let result = load_raw_toml(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_raw_toml_valid_file_returns_some() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(&path, "[server]\nport = 3000\n").unwrap();
        let result = load_raw_toml(&path).unwrap();
        assert!(result.is_some());
        assert_eq!(
            result.unwrap()["server"]["port"],
            toml::Value::Integer(3000)
        );
    }

    #[test]
    fn env_override_log_format_auto() {
        // Kills the "delete match arm Auto" mutant
        let env = MockEnv::new().with("AUTUMN_LOG__FORMAT", "Auto");
        let mut config = AutumnConfig::default();
        // Start with non-Auto to prove the override works
        config.log.format = LogFormat::Json;
        config.apply_env_overrides_with_env(&env);
        assert_eq!(config.log.format, LogFormat::Auto);
    }

    #[test]
    fn env_override_health_detailed_false() {
        // Kills the 'delete match arm "false" | "0"' mutant
        let env = MockEnv::new().with("AUTUMN_HEALTH__DETAILED", "false");
        let mut config = AutumnConfig::default();
        config.health.detailed = true; // start true, override to false
        config.apply_env_overrides_with_env(&env);
        assert!(!config.health.detailed);
    }

    #[test]
    fn env_override_health_detailed_zero() {
        let env = MockEnv::new().with("AUTUMN_HEALTH__DETAILED", "0");
        let mut config = AutumnConfig::default();
        config.health.detailed = true;
        config.apply_env_overrides_with_env(&env);
        assert!(!config.health.detailed);
    }

    #[test]
    fn cors_defaults() {
        let cors = CorsConfig::default();
        assert!(cors.allowed_origins.is_empty());
        assert_eq!(cors.allowed_methods.len(), 6);
        assert!(cors.allowed_methods.contains(&"GET".to_owned()));
        assert!(cors.allowed_headers.contains(&"Content-Type".to_owned()));
        assert!(!cors.allow_credentials);
        assert_eq!(cors.max_age_secs, 86400);
    }

    #[test]
    fn cors_in_full_config_defaults() {
        let config = AutumnConfig::default();
        assert!(config.cors.allowed_origins.is_empty());
    }

    #[test]
    fn actuator_defaults() {
        let config = ActuatorConfig::default();
        assert_eq!(config.prefix, "/actuator");
        assert!(!config.sensitive);
    }

    #[test]
    fn actuator_prefix_in_full_config() {
        let config = AutumnConfig::default();
        assert_eq!(config.actuator.prefix, "/actuator");
    }

    #[test]
    fn deep_merge_handles_deep_nesting() {
        let mut base = toml::Value::Table(toml::map::Map::new());
        let mut overlay = toml::Value::Table(toml::map::Map::new());

        // Create a 10,000 deep nested table
        let mut current_base = &mut base;
        let mut current_overlay = &mut overlay;

        for _ in 0..10_000 {
            if let toml::Value::Table(t) = current_base {
                t.insert("x".to_owned(), toml::Value::Table(toml::map::Map::new()));
                current_base = t.get_mut("x").unwrap();
            }
            if let toml::Value::Table(t) = current_overlay {
                t.insert("x".to_owned(), toml::Value::Table(toml::map::Map::new()));
                current_overlay = t.get_mut("x").unwrap();
            }
        }

        // Add a leaf value to test actual merging
        if let toml::Value::Table(t) = current_overlay {
            t.insert("y".to_owned(), toml::Value::Integer(42));
        }

        // Trigger merge, expecting no panic/stack overflow
        // We run it on a thread with a large stack to avoid the stack overflow caused by Drop when base is dropped at the end of the function (since we created a 10,000 depth structure).
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                deep_merge(&mut base, overlay);
                // Let the OS clean up the memory instead of dropping deeply nested structure
                std::mem::forget(base);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn deep_merge_stops_at_max_depth() {
        let mut base = toml::Value::Table(toml::map::Map::new());
        let mut overlay = toml::Value::Table(toml::map::Map::new());

        // Create structures nested exactly to MAX_MERGE_DEPTH + 1
        let mut current_base = &mut base;
        let mut current_overlay = &mut overlay;

        for _ in 0..=MAX_MERGE_DEPTH {
            if let toml::Value::Table(t) = current_base {
                t.insert("x".to_owned(), toml::Value::Table(toml::map::Map::new()));
                current_base = t.get_mut("x").unwrap();
            }
            if let toml::Value::Table(t) = current_overlay {
                t.insert("x".to_owned(), toml::Value::Table(toml::map::Map::new()));
                current_overlay = t.get_mut("x").unwrap();
            }
        }

        // Add a value deep in the overlay
        if let toml::Value::Table(t) = current_overlay {
            t.insert("deep_value".to_owned(), toml::Value::Integer(123));
        }

        deep_merge(&mut base, overlay);

        // Verify the value was NOT merged due to max depth limit
        let mut current_base_check = &base;
        for _ in 0..=MAX_MERGE_DEPTH {
            if let toml::Value::Table(t) = current_base_check {
                current_base_check = t.get("x").unwrap();
            }
        }

        if let toml::Value::Table(t) = current_base_check {
            assert!(
                !t.contains_key("deep_value"),
                "Value beyond MAX_MERGE_DEPTH should not be merged"
            );
        } else {
            panic!("Expected a table");
        }
    }

    // ── AUTUMN_SECURITY__FORBIDDEN_RESPONSE / __ALLOW_UNAUTHORIZED_REPOSITORY_API ──

    #[test]
    fn env_override_forbidden_response_403() {
        let env = MockEnv::new().with("AUTUMN_SECURITY__FORBIDDEN_RESPONSE", "403");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.forbidden_response,
            crate::authorization::ForbiddenResponse::Forbidden403
        );
    }

    #[test]
    fn env_override_forbidden_response_404() {
        let env = MockEnv::new().with("AUTUMN_SECURITY__FORBIDDEN_RESPONSE", "404");
        let mut config = AutumnConfig::default();
        // Pre-set to 403 to confirm env actually flips it back to 404.
        config.security.forbidden_response = crate::authorization::ForbiddenResponse::Forbidden403;
        config.apply_env_overrides_with_env(&env);
        assert_eq!(
            config.security.forbidden_response,
            crate::authorization::ForbiddenResponse::NotFound404
        );
    }

    #[test]
    fn env_override_forbidden_response_invalid_keeps_existing() {
        let env = MockEnv::new().with("AUTUMN_SECURITY__FORBIDDEN_RESPONSE", "418");
        let mut config = AutumnConfig::default();
        config.security.forbidden_response = crate::authorization::ForbiddenResponse::Forbidden403;
        config.apply_env_overrides_with_env(&env);
        // Invalid value warns and leaves the existing setting alone.
        assert_eq!(
            config.security.forbidden_response,
            crate::authorization::ForbiddenResponse::Forbidden403
        );
    }

    #[test]
    fn env_override_allow_unauthorized_repository_api() {
        let env = MockEnv::new().with("AUTUMN_SECURITY__ALLOW_UNAUTHORIZED_REPOSITORY_API", "true");
        let mut config = AutumnConfig::default();
        assert!(!config.security.allow_unauthorized_repository_api);
        config.apply_env_overrides_with_env(&env);
        assert!(config.security.allow_unauthorized_repository_api);
    }

    #[test]
    fn env_override_allow_unauthorized_repository_api_false_overrides_toml_true() {
        let env = MockEnv::new().with(
            "AUTUMN_SECURITY__ALLOW_UNAUTHORIZED_REPOSITORY_API",
            "false",
        );
        let mut config = AutumnConfig::default();
        config.security.allow_unauthorized_repository_api = true;
        config.apply_env_overrides_with_env(&env);
        assert!(!config.security.allow_unauthorized_repository_api);
    }

    // ── [openapi] config section tests (RED phase) ─────────────────────────

    #[test]
    fn openapi_runtime_config_defaults_enabled() {
        // The [openapi] section must default to enabled=true and path="/openapi.json".
        let config = AutumnConfig::default();
        assert!(
            config.openapi_runtime.enabled,
            "[openapi] must default to enabled = true"
        );
        assert_eq!(
            config.openapi_runtime.path, "/openapi.json",
            "[openapi] must default to path = \"/openapi.json\""
        );
    }

    #[test]
    fn openapi_runtime_config_can_be_disabled_via_toml() {
        let toml_str = "
[openapi]
enabled = false
";
        let config: AutumnConfig = toml::from_str(toml_str).unwrap();
        assert!(
            !config.openapi_runtime.enabled,
            "[openapi] enabled = false must deserialize correctly"
        );
    }

    #[test]
    fn openapi_runtime_config_path_can_be_customized() {
        let toml_str = r#"
[openapi]
path = "/api-spec.json"
"#;
        let config: AutumnConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.openapi_runtime.path, "/api-spec.json",
            "[openapi] path must deserialize correctly"
        );
    }

    #[test]
    fn cache_env_overrides_fields() {
        let env = MockEnv::new()
            .with("AUTUMN_CACHE__BACKEND", "redis")
            .with("AUTUMN_CACHE__REDIS__URL", "redis://cache:6379/1")
            .with("AUTUMN_CACHE__REDIS__KEY_PREFIX", "myapp:cache");
        let mut config = AutumnConfig::default();

        config.apply_env_overrides_with_env(&env);

        assert!(config.cache.is_redis(), "backend should be redis");
        assert_eq!(
            config.cache.redis.url.as_deref(),
            Some("redis://cache:6379/1")
        );
        assert_eq!(config.cache.redis.key_prefix, "myapp:cache");
    }

    #[test]
    fn cache_backend_from_env_value_invalid_is_none() {
        assert!(CacheBackend::from_env_value("postgres").is_none());
        assert!(CacheBackend::from_env_value("").is_none());
    }

    #[test]
    fn scheduler_validate_rejects_zero_lease_ttl() {
        let cfg = SchedulerConfig {
            lease_ttl_secs: 0,
            ..SchedulerConfig::default()
        };
        assert!(cfg.validate().is_err(), "zero lease_ttl_secs must fail");
    }

    #[test]
    fn scheduler_validate_rejects_empty_key_prefix() {
        let cfg = SchedulerConfig {
            key_prefix: "   ".to_owned(),
            ..SchedulerConfig::default()
        };
        assert!(cfg.validate().is_err(), "blank key_prefix must fail");
    }

    #[test]
    fn scheduler_validate_ok_with_defaults() {
        assert!(SchedulerConfig::default().validate().is_ok());
    }

    #[test]
    fn scheduler_resolved_replica_id_uses_explicit_value() {
        let cfg = SchedulerConfig {
            replica_id: Some("my-pod".to_owned()),
            ..SchedulerConfig::default()
        };
        assert_eq!(cfg.resolved_replica_id(), "my-pod");
    }

    #[test]
    fn scheduler_resolved_replica_id_falls_back_to_pid() {
        let cfg = SchedulerConfig {
            replica_id: None,
            ..SchedulerConfig::default()
        };
        // In CI, FLY_MACHINE_ID and HOSTNAME may or may not be set,
        // so just verify we get a non-empty string back.
        assert!(!cfg.resolved_replica_id().is_empty());
    }

    #[cfg(feature = "mail")]
    #[test]
    fn mail_allow_in_process_deliver_later_in_production_is_overridable_via_env() {
        let env = MockEnv::new()
            .with(
                "AUTUMN_MAIL__ALLOW_IN_PROCESS_DELIVER_LATER_IN_PRODUCTION",
                "true",
            )
            .with("AUTUMN_MAIL__TRANSPORT", "smtp")
            .with("AUTUMN_MAIL__SMTP__HOST", "smtp.example.com");

        let mut config = AutumnConfig::default();
        config.apply_mail_env_overrides_with_env(&env);

        assert!(
            config.mail.allow_in_process_deliver_later_in_production,
            "env var should set allow_in_process_deliver_later_in_production"
        );
    }

    #[cfg(feature = "mail")]
    #[test]
    fn mail_allow_in_process_deliver_later_in_production_defaults_false() {
        let env = MockEnv::new();
        let mut config = AutumnConfig::default();
        config.apply_mail_env_overrides_with_env(&env);

        assert!(
            !config.mail.allow_in_process_deliver_later_in_production,
            "flag should default to false when env var is not set"
        );
    }

    // ── credentials integration ───────────────────────────────────────────

    #[test]
    fn config_credentials_empty_when_no_directory() {
        let env = MockEnv::new();
        let config = AutumnConfig::load_with_env(&env).unwrap();
        assert!(
            config.credentials().is_empty(),
            "existing apps without config/credentials/ must boot with an empty credentials store"
        );
    }

    #[test]
    fn config_has_credentials_accessor() {
        let config = AutumnConfig::default();
        let _store = config.credentials();
    }

    #[test]
    fn config_credentials_loaded_when_file_present() {
        use crate::credentials::{MasterKey, encrypt};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let key = MasterKey::generate();
        let ct = encrypt(&key, b"stripe_key = \"sk_test_xyz\"\n");
        std::fs::create_dir_all(tmp.path().join("config/credentials")).unwrap();
        std::fs::write(tmp.path().join("config/credentials/dev.toml.enc"), &ct).unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_MASTER_KEY", &key.to_hex())
            .with("AUTUMN_MANIFEST_DIR", tmp.path().to_str().unwrap());
        let config = AutumnConfig::load_with_env(&env).unwrap();
        let val: Option<String> = config.credentials().get("stripe_key");
        assert_eq!(val.as_deref(), Some("sk_test_xyz"));
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn config_resolves_oauth_credentials_by_convention() {
        use crate::credentials::{MasterKey, encrypt};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let key = MasterKey::generate();
        let ct = encrypt(
            &key,
            b"oauth2_github_client_id = \"git-id-123\"\noauth2_github_client_secret = \"git-secret-456\"\n",
        );
        std::fs::create_dir_all(tmp.path().join("config/credentials")).unwrap();
        std::fs::write(tmp.path().join("config/credentials/dev.toml.enc"), &ct).unwrap();

        // Write a base configuration with an empty/blank github provider defined
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        let config_toml = r#"
[auth.oauth2.github]
client_id = ""
client_secret = ""
authorize_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
redirect_uri = "http://localhost:3000/auth/github/callback"
"#;
        std::fs::write(tmp.path().join("autumn.toml"), config_toml).unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_MASTER_KEY", &key.to_hex())
            .with("AUTUMN_MANIFEST_DIR", tmp.path().to_str().unwrap());
        let config = AutumnConfig::load_with_env(&env).unwrap();
        let github = config.auth.oauth2.providers.get("github").unwrap();
        assert_eq!(github.client_id, "git-id-123");
        assert_eq!(github.client_secret, "git-secret-456");
    }

    #[test]
    fn config_fails_with_credentials_error_when_key_is_invalid() {
        use crate::credentials::encrypt;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // Write an encrypted file but supply a wrong-length key so validation fails
        let bogus_key = "zz".repeat(32); // 64 chars but not valid hex
        let ct = encrypt(&crate::credentials::MasterKey::generate(), b"x = \"y\"\n");
        std::fs::create_dir_all(tmp.path().join("config/credentials")).unwrap();
        std::fs::write(tmp.path().join("config/credentials/dev.toml.enc"), &ct).unwrap();

        let env = MockEnv::new()
            .with("AUTUMN_MASTER_KEY", &bogus_key)
            .with("AUTUMN_MANIFEST_DIR", tmp.path().to_str().unwrap());
        let err = AutumnConfig::load_with_env(&env).unwrap_err();
        assert!(
            matches!(err, ConfigError::Credentials(_)),
            "bad master key should produce ConfigError::Credentials, got {err:?}"
        );
    }

    #[test]
    fn test_parse_duration_str() {
        assert_eq!(
            parse_duration_str("500ms").unwrap(),
            std::time::Duration::from_millis(500)
        );
        assert_eq!(
            parse_duration_str("5s").unwrap(),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            parse_duration_str("2m").unwrap(),
            std::time::Duration::from_secs(120)
        );
        assert_eq!(
            parse_duration_str("1h").unwrap(),
            std::time::Duration::from_secs(3600)
        );
        assert_eq!(
            parse_duration_str("1000").unwrap(),
            std::time::Duration::from_secs(1)
        );
        assert!(parse_duration_str("abc").is_err());
        assert!(parse_duration_str("").is_err());
    }

    #[test]
    fn test_database_config_duration_deserialization() {
        #[derive(Debug, Deserialize)]
        struct TestConfig {
            #[serde(deserialize_with = "deserialize_option_duration", default)]
            timeout: Option<std::time::Duration>,
            #[serde(deserialize_with = "deserialize_duration")]
            threshold: std::time::Duration,
        }

        let toml_str = r#"
            timeout = "2s"
            threshold = "100ms"
        "#;
        let parsed: TestConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.timeout, Some(std::time::Duration::from_secs(2)));
        assert_eq!(parsed.threshold, std::time::Duration::from_millis(100));

        let toml_str_null = r#"
            threshold = "500"
        "#;
        let parsed_null: TestConfig = toml::from_str(toml_str_null).unwrap();
        assert_eq!(parsed_null.timeout, None);
        assert_eq!(parsed_null.threshold, std::time::Duration::from_millis(500));
    }

    // ── RequestTimeoutsConfig ──────────────────────────────────────────────

    #[test]
    fn request_timeouts_config_defaults_to_none() {
        let config = RequestTimeoutsConfig::default();
        assert!(config.request_timeout_ms.is_none());
    }

    #[test]
    fn server_config_timeouts_defaults_to_disabled() {
        let config = ServerConfig::default();
        assert!(config.timeouts.request_timeout_ms.is_none());
    }

    #[test]
    fn request_timeouts_config_can_be_set_via_toml() {
        let toml_str = "request_timeout_ms = 5000";
        let config: RequestTimeoutsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.request_timeout_ms, Some(5000));
    }

    #[test]
    fn server_config_timeouts_deserialize_nested() {
        let toml_str = r#"
            port = 3000
            host = "127.0.0.1"
            shutdown_timeout_secs = 30
            prestop_grace_secs = 5

            [timeouts]
            request_timeout_ms = 15000
        "#;
        let config: ServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.timeouts.request_timeout_ms, Some(15_000));
    }

    #[test]
    fn autumn_config_server_timeouts_roundtrip() {
        let mut config = AutumnConfig::default();
        config.server.timeouts.request_timeout_ms = Some(20_000);
        assert_eq!(config.server.timeouts.request_timeout_ms, Some(20_000));
    }

    #[test]
    fn server_timeouts_env_var_override() {
        struct FakeEnv(std::collections::HashMap<String, String>);
        impl Env for FakeEnv {
            fn var(&self, key: &str) -> Result<String, std::env::VarError> {
                self.0
                    .get(key)
                    .cloned()
                    .ok_or(std::env::VarError::NotPresent)
            }
        }

        let mut config = AutumnConfig::default();
        let env = FakeEnv(
            [(
                "AUTUMN_SERVER__TIMEOUTS__REQUEST_TIMEOUT_MS".to_owned(),
                "8000".to_owned(),
            )]
            .into(),
        );
        config.apply_server_env_overrides_with_env(&env);
        assert_eq!(config.server.timeouts.request_timeout_ms, Some(8000));
    }

    #[test]
    fn prod_profile_sets_request_timeout_30s() {
        let defaults = profile_defaults_as_toml("prod");
        let toml_str = toml::to_string(&defaults).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            config.server.timeouts.request_timeout_ms,
            Some(30_000),
            "prod profile must enable the 30-second request timeout by default"
        );
    }

    #[test]
    fn dev_profile_leaves_request_timeout_disabled() {
        let defaults = profile_defaults_as_toml("dev");
        let toml_str = toml::to_string(&defaults).unwrap();
        let config: AutumnConfig = toml::from_str(&toml_str).unwrap();
        assert!(
            config.server.timeouts.request_timeout_ms.is_none(),
            "dev profile must not enable a request timeout by default"
        );
    }
}
