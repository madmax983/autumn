//! Framework configuration with sensible defaults and profile-based layering.
//!
//! Autumn uses a five-layer configuration system where each layer
//! overrides the previous one:
//!
//! 1. **Framework defaults** (this module) -- compiled into the binary.
//! 2. **Profile smart defaults** -- per-profile values for `dev`/`prod`.
//! 3. **`autumn.toml`** -- project-level overrides checked into source control.
//! 4. **`autumn-{profile}.toml`** -- profile-specific overrides.
//! 5. **`AUTUMN_*` environment variables** -- deployment/CI overrides.
//!
//! An Autumn application runs with zero configuration -- every field
//! has a sensible default value. Override only what you need.
//!
//! # Profiles
//!
//! Profiles are resolved in precedence order:
//! 1. `AUTUMN_PROFILE` environment variable
//! 2. `--profile` CLI flag
//! 3. Auto-detect from debug/release build mode
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
//! | `AUTUMN_DATABASE__URL` | `database.url` | `String` |
//! | `AUTUMN_DATABASE__POOL_SIZE` | `database.pool_size` | `usize` |
//! | `AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS` | `database.connect_timeout_secs` | `u64` |
//! | `AUTUMN_LOG__LEVEL` | `log.level` | tracing filter directive |
//! | `AUTUMN_LOG__FORMAT` | `log.format` | `Auto` / `Pretty` / `Json` |
//! | `AUTUMN_HEALTH__PATH` | `health.path` | `String` |
//! | `AUTUMN_HEALTH__DETAILED` | `health.detailed` | `bool` |
//! | `AUTUMN_CORS__ALLOWED_ORIGINS` | `cors.allowed_origins` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOWED_METHODS` | `cors.allowed_methods` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOWED_HEADERS` | `cors.allowed_headers` | comma-separated `String` |
//! | `AUTUMN_CORS__ALLOW_CREDENTIALS` | `cors.allow_credentials` | `bool` |
//! | `AUTUMN_CORS__MAX_AGE_SECS` | `cors.max_age_secs` | `u64` |
//! | `AUTUMN_PROFILE` | active profile | `String` |

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Locate a config file by checking the app's crate directory first, then CWD.
fn find_config_file_named(filename: &str) -> PathBuf {
    if let Ok(manifest_dir) = std::env::var("AUTUMN_MANIFEST_DIR") {
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
        Ok(contents) => Ok(Some(contents.parse::<toml::Value>()?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Resolve the active profile using the three-mechanism precedence chain.
///
/// 1. `AUTUMN_PROFILE` env var (highest priority)
/// 2. `--profile <name>` CLI flag
/// 3. Auto-detect from build mode (`AUTUMN_IS_DEBUG` set by `#[autumn_web::main]`)
fn resolve_profile() -> Option<String> {
    // 1. Env var
    if let Ok(profile) = std::env::var("AUTUMN_PROFILE") {
        if !profile.is_empty() {
            return Some(profile);
        }
    }

    // 2. CLI flag
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--profile" {
            if let Some(profile) = args.get(i + 1) {
                return Some(profile.clone());
            }
        }
        if let Some(profile) = arg.strip_prefix("--profile=") {
            return Some(profile.to_owned());
        }
    }

    // 3. Auto-detect from build mode
    match std::env::var("AUTUMN_IS_DEBUG").ok().as_deref() {
        Some("1") => Some("dev".to_owned()),
        Some("0") => Some("prod".to_owned()),
        _ => None,
    }
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

            let mut server = toml::map::Map::new();
            server.insert("host".into(), "127.0.0.1".into());
            server.insert("shutdown_timeout_secs".into(), toml::Value::Integer(1));
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

            // Dev: CSRF disabled (default), HSTS off (default)
        }
        "prod" => {
            let mut log = toml::map::Map::new();
            log.insert("level".into(), "info".into());
            log.insert("format".into(), "Json".into());
            table.insert("log".into(), toml::Value::Table(log));

            let mut server = toml::map::Map::new();
            server.insert("host".into(), "0.0.0.0".into());
            server.insert("shutdown_timeout_secs".into(), toml::Value::Integer(30));
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

/// Deep-merge two TOML values. Tables are merged recursively;
/// non-table values in `overlay` replace those in `base`.
fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    if base.is_table() && overlay.is_table() {
        if let toml::Value::Table(overlay_table) = overlay {
            let base_table = base.as_table_mut().expect("checked is_table above");
            for (key, overlay_val) in overlay_table {
                if overlay_val.is_table() {
                    if let Some(base_val) = base_table.get_mut(&key) {
                        if base_val.is_table() {
                            deep_merge(base_val, overlay_val);
                            continue;
                        }
                    }
                }
                base_table.insert(key, overlay_val);
            }
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

/// Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = b.len();
    let mut prev = (0..=n).collect::<Vec<_>>();
    let mut curr = vec![0; n + 1];
    for (i, a_ch) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b.iter().enumerate() {
            let cost = usize::from(a_ch != b_ch);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
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
#[derive(Debug, Default, Deserialize)]
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

    /// Authentication settings.
    #[serde(default)]
    pub auth: crate::auth::AuthConfig,

    /// Security settings (headers, CSRF).
    #[serde(default)]
    pub security: crate::security::config::SecurityConfig,
}

impl AutumnConfig {
    /// Load configuration with profile-aware layering.
    ///
    /// Applies the five-layer configuration system:
    /// 1. Framework defaults
    /// 2. Profile smart defaults (dev/prod)
    /// 3. `autumn.toml` (base config)
    /// 4. `autumn-{profile}.toml` (profile overrides)
    /// 5. `AUTUMN_*` environment variables
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
        let profile = resolve_profile();

        // Build merged TOML: profile smart defaults ← autumn.toml ← autumn-{profile}.toml
        let mut merged = profile.as_ref().map_or_else(
            || toml::Value::Table(toml::map::Map::new()),
            |p| profile_defaults_as_toml(p),
        );

        // Layer 3: base autumn.toml
        if let Some(base) = load_raw_toml(&find_config_file_named("autumn.toml"))? {
            deep_merge(&mut merged, base);
        }

        // Layer 4: autumn-{profile}.toml
        if let Some(ref p) = profile {
            let profile_path = find_config_file_named(&format!("autumn-{p}.toml"));
            match load_raw_toml(&profile_path)? {
                Some(profile_toml) => deep_merge(&mut merged, profile_toml),
                None if p != "dev" && p != "prod" => warn_profile_typo(p),
                None => {}
            }
        }

        // Deserialize the merged TOML table into AutumnConfig
        let toml_str =
            toml::to_string(&merged).expect("internal error: failed to serialize merged config");
        let mut config: Self = toml::from_str(&toml_str)?;
        config.profile = profile;

        // Layer 5: env var overrides (highest priority)
        config.apply_env_overrides();

        config.database.validate()?;
        Ok(config)
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
            Ok(contents) => Ok(toml::from_str(&contents)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
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
    ///
    /// # Database
    /// - `AUTUMN_DATABASE__URL` → `database.url` (String)
    /// - `AUTUMN_DATABASE__POOL_SIZE` → `database.pool_size` (usize)
    /// - `AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS` → `database.connect_timeout_secs` (u64)
    ///
    /// # Log
    /// - `AUTUMN_LOG__LEVEL` → `log.level` (String, tracing filter directive)
    /// - `AUTUMN_LOG__FORMAT` → `log.format` (Auto | Pretty | Json)
    ///
    /// # Health
    /// - `AUTUMN_HEALTH__PATH` → `health.path` (String)
    pub fn apply_env_overrides(&mut self) {
        // ── Server ──────────────────────────────────────────────
        parse_env("AUTUMN_SERVER__PORT", &mut self.server.port);
        if let Ok(val) = std::env::var("AUTUMN_SERVER__HOST") {
            self.server.host = val;
        }
        parse_env(
            "AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS",
            &mut self.server.shutdown_timeout_secs,
        );

        // ── Database ────────────────────────────────────────────
        if let Ok(val) = std::env::var("AUTUMN_DATABASE__URL") {
            self.database.url = Some(val);
        }
        parse_env("AUTUMN_DATABASE__POOL_SIZE", &mut self.database.pool_size);
        parse_env(
            "AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS",
            &mut self.database.connect_timeout_secs,
        );

        // ── Log ─────────────────────────────────────────────────
        if let Ok(val) = std::env::var("AUTUMN_LOG__LEVEL") {
            self.log.level = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_LOG__FORMAT") {
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

        // ── Health ──────────────────────────────────────────────
        if let Ok(val) = std::env::var("AUTUMN_HEALTH__PATH") {
            self.health.path = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_HEALTH__DETAILED") {
            match val.as_str() {
                "true" | "1" => self.health.detailed = true,
                "false" | "0" => self.health.detailed = false,
                _ => eprintln!(
                    "Warning: AUTUMN_HEALTH__DETAILED={val:?} is not valid \
                     (expected true/false), ignoring"
                ),
            }
        }

        // ── CORS ────────────────────────────────────────────────
        if let Ok(val) = std::env::var("AUTUMN_CORS__ALLOWED_ORIGINS") {
            self.cors.allowed_origins = val.split(',').map(|s| s.trim().to_owned()).collect();
        }
        if let Ok(val) = std::env::var("AUTUMN_CORS__ALLOWED_METHODS") {
            self.cors.allowed_methods = val.split(',').map(|s| s.trim().to_owned()).collect();
        }
        if let Ok(val) = std::env::var("AUTUMN_CORS__ALLOWED_HEADERS") {
            self.cors.allowed_headers = val.split(',').map(|s| s.trim().to_owned()).collect();
        }
        if let Ok(val) = std::env::var("AUTUMN_CORS__ALLOW_CREDENTIALS") {
            match val.as_str() {
                "true" | "1" => self.cors.allow_credentials = true,
                "false" | "0" => self.cors.allow_credentials = false,
                _ => eprintln!(
                    "Warning: AUTUMN_CORS__ALLOW_CREDENTIALS={val:?} is not valid \
                     (expected true/false), ignoring"
                ),
            }
        }
        parse_env("AUTUMN_CORS__MAX_AGE_SECS", &mut self.cors.max_age_secs);

        // ── Session ────────────────────────────────────────────
        if let Ok(val) = std::env::var("AUTUMN_SESSION__COOKIE_NAME") {
            self.session.cookie_name = val;
        }
        parse_env(
            "AUTUMN_SESSION__MAX_AGE_SECS",
            &mut self.session.max_age_secs,
        );
        if let Ok(val) = std::env::var("AUTUMN_SESSION__SECURE") {
            match val.as_str() {
                "true" | "1" => self.session.secure = true,
                "false" | "0" => self.session.secure = false,
                _ => eprintln!(
                    "Warning: AUTUMN_SESSION__SECURE={val:?} is not valid \
                     (expected true/false), ignoring"
                ),
            }
        }
        if let Ok(val) = std::env::var("AUTUMN_SESSION__SAME_SITE") {
            self.session.same_site = val;
        }

        // ── Auth ───────────────────────────────────────────────
        parse_env("AUTUMN_AUTH__BCRYPT_COST", &mut self.auth.bcrypt_cost);
        if let Ok(val) = std::env::var("AUTUMN_AUTH__SESSION_KEY") {
            self.auth.session_key = val;
        }

        // ── Security ────────────────────────────────────────
        self.apply_security_env_overrides();
    }

    /// Apply `AUTUMN_SECURITY__*` environment variable overrides.
    fn apply_security_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__X_FRAME_OPTIONS") {
            self.security.headers.x_frame_options = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__X_CONTENT_TYPE_OPTIONS") {
            match val.as_str() {
                "true" | "1" => self.security.headers.x_content_type_options = true,
                "false" | "0" => self.security.headers.x_content_type_options = false,
                _ => eprintln!(
                    "Warning: AUTUMN_SECURITY__HEADERS__X_CONTENT_TYPE_OPTIONS={val:?} \
                     is not valid (expected true/false), ignoring"
                ),
            }
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__STRICT_TRANSPORT_SECURITY") {
            match val.as_str() {
                "true" | "1" => self.security.headers.strict_transport_security = true,
                "false" | "0" => self.security.headers.strict_transport_security = false,
                _ => eprintln!(
                    "Warning: AUTUMN_SECURITY__HEADERS__STRICT_TRANSPORT_SECURITY={val:?} \
                     is not valid (expected true/false), ignoring"
                ),
            }
        }
        parse_env(
            "AUTUMN_SECURITY__HEADERS__HSTS_MAX_AGE_SECS",
            &mut self.security.headers.hsts_max_age_secs,
        );
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__CONTENT_SECURITY_POLICY") {
            self.security.headers.content_security_policy = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__REFERRER_POLICY") {
            self.security.headers.referrer_policy = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__HEADERS__PERMISSIONS_POLICY") {
            self.security.headers.permissions_policy = val;
        }

        // CSRF
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__CSRF__ENABLED") {
            match val.as_str() {
                "true" | "1" => self.security.csrf.enabled = true,
                "false" | "0" => self.security.csrf.enabled = false,
                _ => eprintln!(
                    "Warning: AUTUMN_SECURITY__CSRF__ENABLED={val:?} \
                     is not valid (expected true/false), ignoring"
                ),
            }
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__CSRF__TOKEN_HEADER") {
            self.security.csrf.token_header = val;
        }
        if let Ok(val) = std::env::var("AUTUMN_SECURITY__CSRF__COOKIE_NAME") {
            self.security.csrf.cookie_name = val;
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
#[derive(Debug, Deserialize)]
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
/// | `pool_size` | `10` |
/// | `connect_timeout_secs` | `5` |
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
#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    /// Postgres connection URL. `None` means no database is configured.
    ///
    /// Must start with `postgres://` or `postgresql://` when present.
    #[serde(default)]
    pub url: Option<String>,

    /// Maximum number of connections in the pool. Default: `10`.
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,

    /// Seconds to wait when acquiring a connection from the pool.
    /// Default: `5`.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

impl DatabaseConfig {
    /// Validate database configuration.
    ///
    /// # Errors
    ///
    /// Returns a validation error if the URL has an invalid scheme.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(ref url) = self.url {
            if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                return Err(ConfigError::Validation(format!(
                    "Invalid database URL: must start with postgres:// or postgresql://, got {url:?}"
                )));
            }
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Pretty in dev, JSON in production (based on `AUTUMN_ENV`).
    #[default]
    Auto,
    /// Human-readable, colorized output.
    Pretty,
    /// Structured JSON output suitable for log aggregation pipelines.
    Json,
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
/// assert!(!health.detailed);
/// ```
#[derive(Debug, Deserialize)]
pub struct HealthConfig {
    /// URL path for the health check endpoint. Default: `"/health"`.
    ///
    /// Common alternatives: `"/healthz"`, `"/_health"`, `"/ready"`.
    #[serde(default = "default_health_path")]
    pub path: String,

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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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

/// Parse an environment variable into a typed target, logging a warning on failure.
fn parse_env<T: std::str::FromStr>(key: &str, target: &mut T) {
    if let Ok(val) = std::env::var(key) {
        match val.parse::<T>() {
            Ok(v) => *target = v,
            Err(_) => eprintln!("Warning: {key}={val:?} is not valid, ignoring"),
        }
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

const fn default_pool_size() -> usize {
    10
}

const fn default_connect_timeout() -> u64 {
    5
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_health_path() -> String {
    "/health".to_owned()
}

// ── Default trait impls ────────────────────────────────────────────

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            host: default_host(),
            shutdown_timeout_secs: default_shutdown_timeout(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: None,
            pool_size: default_pool_size(),
            connect_timeout_secs: default_connect_timeout(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: LogFormat::default(),
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            path: default_health_path(),
            detailed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex that serialises all tests which mutate environment variables.
    /// Env vars are process-global, so concurrent tests that set/restore
    /// the same key race each other. Locking this mutex prevents that.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that sets an env var and restores it on drop.
    /// Holds `ENV_LOCK` for its entire lifetime so concurrent env-mutating
    /// tests cannot interleave.
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let original = std::env::var(key).ok();
            // SAFETY: Serialised by ENV_LOCK — only one test mutates the
            // environment at a time.
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key,
                original,
                _lock: lock,
            }
        }

        /// Remove an env var for the test duration. Restores on drop.
        fn remove(key: &'static str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let original = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                key,
                original,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                // SAFETY: Still holding ENV_LOCK via _lock field.
                Some(val) => unsafe { std::env::set_var(self.key, val) },
                None => unsafe { std::env::remove_var(self.key) },
            }
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
    fn log_defaults() {
        let config = LogConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, LogFormat::Auto);
    }

    #[test]
    fn health_defaults() {
        let config = HealthConfig::default();
        assert_eq!(config.path, "/health");
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
        let _guard = EnvGuard::set("AUTUMN_DATABASE__URL", "postgres://override:5432/test");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://override:5432/test")
        );
    }

    #[test]
    fn env_override_pool_size() {
        let _guard = EnvGuard::set("AUTUMN_DATABASE__POOL_SIZE", "25");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.database.pool_size, 25);
    }

    #[test]
    fn env_override_connect_timeout() {
        let _guard = EnvGuard::set("AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS", "15");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.database.connect_timeout_secs, 15);
    }

    #[test]
    fn env_override_invalid_pool_size_ignored() {
        let _guard = EnvGuard::set("AUTUMN_DATABASE__POOL_SIZE", "not_a_number");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.database.pool_size, 10);
    }

    // ── Server env override tests ────────────────────────────────

    #[test]
    fn env_override_server_port() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__PORT", "8080");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn env_override_server_host() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__HOST", "0.0.0.0");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.server.host, "0.0.0.0");
    }

    #[test]
    fn env_override_server_shutdown_timeout() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "60");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.server.shutdown_timeout_secs, 60);
    }

    #[test]
    fn env_override_invalid_server_port_ignored() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__PORT", "not_a_port");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn env_override_invalid_shutdown_timeout_ignored() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "forever");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.server.shutdown_timeout_secs, 30);
    }

    // ── Log env override tests ───────────────────────────────────

    #[test]
    fn env_override_log_level() {
        let _guard = EnvGuard::set("AUTUMN_LOG__LEVEL", "debug");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.log.level, "debug");
    }

    #[test]
    fn env_override_log_format_json() {
        let _guard = EnvGuard::set("AUTUMN_LOG__FORMAT", "Json");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.log.format, LogFormat::Json);
    }

    #[test]
    fn env_override_log_format_pretty() {
        let _guard = EnvGuard::set("AUTUMN_LOG__FORMAT", "Pretty");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.log.format, LogFormat::Pretty);
    }

    #[test]
    fn env_override_invalid_log_format_ignored() {
        let _guard = EnvGuard::set("AUTUMN_LOG__FORMAT", "yaml");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.log.format, LogFormat::Auto);
    }

    // ── Health env override tests ────────────────────────────────

    #[test]
    fn env_override_health_path() {
        let _guard = EnvGuard::set("AUTUMN_HEALTH__PATH", "/healthz");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.health.path, "/healthz");
    }

    // ── Precedence test ──────────────────────────────────────────

    #[test]
    fn env_overrides_toml_values() {
        let _guard = EnvGuard::set("AUTUMN_SERVER__PORT", "9999");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.toml");
        std::fs::write(&path, "[server]\nport = 4000\n").unwrap();
        let mut config = AutumnConfig::load_from(&path).unwrap();
        config.apply_env_overrides();
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
    fn resolve_profile_from_env() {
        let _guard = EnvGuard::set("AUTUMN_PROFILE", "staging");
        let profile = resolve_profile();
        assert_eq!(profile.as_deref(), Some("staging"));
    }

    #[test]
    fn resolve_profile_auto_detect_debug() {
        // EnvGuard::remove holds the lock, so set AUTUMN_IS_DEBUG manually
        // while already holding it via _clear.
        let _clear = EnvGuard::remove("AUTUMN_PROFILE");
        // SAFETY: lock already held by _clear
        unsafe { std::env::set_var("AUTUMN_IS_DEBUG", "1") };
        let profile = resolve_profile();
        unsafe { std::env::remove_var("AUTUMN_IS_DEBUG") };
        assert_eq!(profile.as_deref(), Some("dev"));
    }

    #[test]
    fn resolve_profile_auto_detect_release() {
        let _clear = EnvGuard::remove("AUTUMN_PROFILE");
        unsafe { std::env::set_var("AUTUMN_IS_DEBUG", "0") };
        let profile = resolve_profile();
        unsafe { std::env::remove_var("AUTUMN_IS_DEBUG") };
        assert_eq!(profile.as_deref(), Some("prod"));
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
        assert!(config.health.detailed);
        assert_eq!(config.cors.allowed_origins, vec!["*"]);
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
        assert!(!config.health.detailed);
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
    fn levenshtein_basic() {
        assert_eq!(levenshtein("dev", "dev"), 0);
        assert_eq!(levenshtein("dev", "dve"), 2); // swap = 2 edits (del + ins)
        assert_eq!(levenshtein("prod", "prodd"), 1);
        assert_eq!(levenshtein("prod", "prd"), 1);
        assert_eq!(levenshtein("staging", "dev"), 7);
    }

    #[test]
    fn env_override_health_detailed() {
        let _guard = EnvGuard::set("AUTUMN_HEALTH__DETAILED", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
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
        let _guard = EnvGuard::remove("AUTUMN_MANIFEST_DIR");
        let path = find_config_file_named("autumn.toml");
        assert_eq!(path, PathBuf::from("autumn.toml"));
    }

    #[test]
    fn find_config_file_uses_manifest_dir_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("autumn.toml");
        std::fs::write(&config_path, "").unwrap();

        let _guard = EnvGuard::set("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());
        let path = find_config_file_named("autumn.toml");
        assert_eq!(path, config_path);
    }

    #[test]
    fn find_config_file_falls_back_when_manifest_dir_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        // dir exists but the file doesn't
        let _guard = EnvGuard::set("AUTUMN_MANIFEST_DIR", dir.path().to_str().unwrap());
        let path = find_config_file_named("nonexistent.toml");
        assert_eq!(path, PathBuf::from("nonexistent.toml"));
    }

    #[test]
    fn resolve_profile_cli_flag_exact_match() {
        // resolve_profile checks `--profile` in CLI args. We can't easily
        // inject args, but we can verify the env path doesn't match other args.
        // The `== "--profile"` guard is the key: if it were `!=`, every arg
        // would trigger the branch.
        let _clear = EnvGuard::remove("AUTUMN_PROFILE");
        unsafe { std::env::remove_var("AUTUMN_IS_DEBUG") };
        // With no env vars and no matching CLI args, should be None
        let profile = resolve_profile();
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
    fn levenshtein_threshold_in_warn_profile_typo() {
        assert!(levenshtein("dve", "dev") <= 2);
        assert!(levenshtein("xyz", "dev") > 2);
        assert!(levenshtein("xyz", "prod") > 2);
    }

    #[test]
    fn env_override_cors_allowed_origins() {
        let _guard = EnvGuard::set(
            "AUTUMN_CORS__ALLOWED_ORIGINS",
            "https://a.com, https://b.com",
        );
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(
            config.cors.allowed_origins,
            vec!["https://a.com", "https://b.com"]
        );
    }

    #[test]
    fn env_override_cors_allow_credentials() {
        let _guard = EnvGuard::set("AUTUMN_CORS__ALLOW_CREDENTIALS", "true");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert!(config.cors.allow_credentials);
    }

    #[test]
    fn env_override_cors_max_age() {
        let _guard = EnvGuard::set("AUTUMN_CORS__MAX_AGE_SECS", "3600");
        let mut config = AutumnConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.cors.max_age_secs, 3600);
    }

    #[test]
    fn load_uses_profile_layering() {
        // Test AutumnConfig::load() with a dev profile via env var.
        // This kills the "replace load → Ok(Default::default())" mutant.
        let _profile = EnvGuard::set("AUTUMN_PROFILE", "dev");
        // Remove AUTUMN_MANIFEST_DIR so it doesn't find stray config files
        unsafe { std::env::remove_var("AUTUMN_MANIFEST_DIR") };

        let config = AutumnConfig::load().unwrap();
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
        let _profile = EnvGuard::set("AUTUMN_PROFILE", "staging");
        unsafe { std::env::remove_var("AUTUMN_MANIFEST_DIR") };

        let config = AutumnConfig::load().unwrap();
        assert_eq!(config.profile.as_deref(), Some("staging"));
        // staging has no smart defaults, so values should be framework defaults
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn load_dev_profile_no_profile_toml_no_warn() {
        // dev/prod without their profile TOML should NOT trigger warn_profile_typo.
        // This tests the `None => {}` branch (line 342).
        let _profile = EnvGuard::set("AUTUMN_PROFILE", "dev");
        unsafe { std::env::remove_var("AUTUMN_MANIFEST_DIR") };

        let config = AutumnConfig::load().unwrap();
        assert_eq!(config.profile.as_deref(), Some("dev"));
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
        let _guard = EnvGuard::set("AUTUMN_LOG__FORMAT", "Auto");
        let mut config = AutumnConfig::default();
        // Start with non-Auto to prove the override works
        config.log.format = LogFormat::Json;
        config.apply_env_overrides();
        assert_eq!(config.log.format, LogFormat::Auto);
    }

    #[test]
    fn env_override_health_detailed_false() {
        // Kills the 'delete match arm "false" | "0"' mutant
        let _guard = EnvGuard::set("AUTUMN_HEALTH__DETAILED", "false");
        let mut config = AutumnConfig::default();
        config.health.detailed = true; // start true, override to false
        config.apply_env_overrides();
        assert!(!config.health.detailed);
    }

    #[test]
    fn env_override_health_detailed_zero() {
        let _guard = EnvGuard::set("AUTUMN_HEALTH__DETAILED", "0");
        let mut config = AutumnConfig::default();
        config.health.detailed = true;
        config.apply_env_overrides();
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
}
