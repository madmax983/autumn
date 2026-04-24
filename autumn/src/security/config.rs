//! Security configuration for Autumn applications.
//!
//! Controls security headers and CSRF protection. All settings have
//! sensible defaults and are profile-aware:
//!
//! - **`dev`**: Relaxed -- CSRF disabled, HSTS off, permissive headers.
//! - **`prod`**: Strict -- CSRF enabled, HSTS on, all protective headers active.
//!
//! Session and authentication configuration live in their own modules
//! ([`crate::session::SessionConfig`], [`crate::auth::AuthConfig`]).
//!
//! # `autumn.toml` example
//!
//! ```toml
//! [security.headers]
//! x_frame_options = "DENY"
//! content_security_policy = "default-src 'self'"
//!
//! [security.csrf]
//! enabled = true
//!
//! [security.rate_limit]
//! enabled = true
//! requests_per_second = 10.0
//! burst = 20
//! ```
//!
//! # Environment variable reference
//!
//! | Variable | Config field | Type |
//! |----------|-------------|------|
//! | `AUTUMN_SECURITY__HEADERS__X_FRAME_OPTIONS` | `security.headers.x_frame_options` | `String` |
//! | `AUTUMN_SECURITY__HEADERS__HSTS_MAX_AGE_SECS` | `security.headers.hsts_max_age_secs` | `u64` |
//! | `AUTUMN_SECURITY__HEADERS__CONTENT_SECURITY_POLICY` | `security.headers.content_security_policy` | `String` |
//! | `AUTUMN_SECURITY__CSRF__ENABLED` | `security.csrf.enabled` | `bool` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__ENABLED` | `security.rate_limit.enabled` | `bool` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__REQUESTS_PER_SECOND` | `security.rate_limit.requests_per_second` | `f64` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__BURST` | `security.rate_limit.burst` | `u32` |
//! | `AUTUMN_SECURITY__RATE_LIMIT__TRUST_FORWARDED_HEADERS` | `security.rate_limit.trust_forwarded_headers` | `bool` |
//!
//! Setting any header value to an empty string disables it (the header is
//! not emitted). This is the escape hatch for opting out of a default.

use serde::Deserialize;

/// Top-level security configuration section.
///
/// Groups security headers and CSRF protection under `[security]`
/// in `autumn.toml`.
///
/// # Examples
///
/// ```rust
/// use autumn_web::security::config::SecurityConfig;
///
/// let config = SecurityConfig::default();
/// assert_eq!(config.headers.x_frame_options, "DENY");
/// assert!(config.headers.x_content_type_options);
/// assert!(!config.csrf.enabled);
/// assert!(!config.rate_limit.enabled);
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecurityConfig {
    /// HTTP security headers applied to all responses.
    #[serde(default)]
    pub headers: HeadersConfig,

    /// CSRF (Cross-Site Request Forgery) protection.
    #[serde(default)]
    pub csrf: CsrfConfig,

    /// Rate limiting (per-client-IP token bucket).
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

/// Security response headers configuration.
///
/// Controls which protective HTTP headers are added to every response.
/// Follows OWASP security header recommendations.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `x_frame_options` | `"DENY"` |
/// | `x_content_type_options` | `true` |
/// | `xss_protection` | `true` |
/// | `strict_transport_security` | `false` |
/// | `hsts_max_age_secs` | `31_536_000` (1 year) |
/// | `hsts_include_subdomains` | `true` |
/// | `content_security_policy` | htmx-compatible policy (see [`default_content_security_policy`]) |
/// | `referrer_policy` | `"strict-origin-when-cross-origin"` |
/// | `permissions_policy` | `""` (disabled) |
///
/// # Examples
///
/// ```toml
/// [security.headers]
/// x_frame_options = "SAMEORIGIN"
/// content_security_policy = "default-src 'self'; script-src 'self'"
/// strict_transport_security = true
/// ```
#[derive(Debug, Clone, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HeadersConfig {
    /// `X-Frame-Options` header value. Default: `"DENY"`.
    ///
    /// Prevents the page from being loaded in an iframe. Common values:
    /// - `"DENY"` -- never allow framing
    /// - `"SAMEORIGIN"` -- allow framing by same origin
    /// - `""` -- do not send the header
    #[serde(default = "default_x_frame_options")]
    pub x_frame_options: String,

    /// Add `X-Content-Type-Options: nosniff`. Default: `true`.
    ///
    /// Prevents MIME-type sniffing attacks.
    #[serde(default = "default_true")]
    pub x_content_type_options: bool,

    /// Add `X-XSS-Protection: 1; mode=block`. Default: `true`.
    ///
    /// Enables the browser's built-in XSS filter (legacy but still useful).
    #[serde(default = "default_true")]
    pub xss_protection: bool,

    /// Add `Strict-Transport-Security` (HSTS) header. Default: `false`.
    ///
    /// When `true`, tells browsers to only connect via HTTPS. Enabled
    /// automatically for `prod` profile via smart defaults.
    #[serde(default)]
    pub strict_transport_security: bool,

    /// HSTS `max-age` in seconds. Default: `31_536_000` (1 year).
    ///
    /// Only used when `strict_transport_security` is `true`.
    #[serde(default = "default_hsts_max_age")]
    pub hsts_max_age_secs: u64,

    /// Include subdomains in HSTS policy. Default: `true`.
    #[serde(default = "default_true")]
    pub hsts_include_subdomains: bool,

    /// `Content-Security-Policy` header value.
    ///
    /// Defaults to an htmx-compatible, same-origin policy (see
    /// [`default_content_security_policy`]). When set to an empty string,
    /// the header is not emitted (explicit opt-out).
    ///
    /// The default allows htmx to function normally because htmx and Autumn's
    /// htmx CSRF helper are served from the same origin and operate via
    /// `addEventListener` rather than inline scripts.
    #[serde(default = "default_content_security_policy")]
    pub content_security_policy: String,

    /// `Referrer-Policy` header value. Default: `"strict-origin-when-cross-origin"`.
    #[serde(default = "default_referrer_policy")]
    pub referrer_policy: String,

    /// `Permissions-Policy` header value. Default: `""` (not sent).
    ///
    /// Controls which browser features and APIs can be used.
    /// Example: `"camera=(), microphone=(), geolocation=()"`.
    #[serde(default)]
    pub permissions_policy: String,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self {
            x_frame_options: default_x_frame_options(),
            x_content_type_options: true,
            xss_protection: true,
            strict_transport_security: false,
            hsts_max_age_secs: default_hsts_max_age(),
            hsts_include_subdomains: true,
            content_security_policy: default_content_security_policy(),
            referrer_policy: default_referrer_policy(),
            permissions_policy: String::new(),
        }
    }
}

/// CSRF (Cross-Site Request Forgery) protection configuration.
///
/// When enabled, mutating requests (POST, PUT, DELETE, PATCH) must include
/// a valid CSRF token either as:
///
/// - An HTTP header (default: `X-CSRF-Token`)
/// - A form field (default: `_csrf`)
///
/// The token is generated per-session and stored in a cookie.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `false` |
/// | `token_header` | `"X-CSRF-Token"` |
/// | `form_field` | `"_csrf"` |
/// | `cookie_name` | `"autumn-csrf"` |
/// | `safe_methods` | `["GET", "HEAD", "OPTIONS", "TRACE"]` |
/// | `exempt_paths` | `[]` |
///
/// # Examples
///
/// ```toml
/// [security.csrf]
/// enabled = true
/// token_header = "X-XSRF-Token"
/// cookie_name = "XSRF-TOKEN"
/// exempt_paths = ["/api/"]
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct CsrfConfig {
    /// Enable CSRF protection. Default: `false`.
    ///
    /// Enabled automatically for `prod` profile via smart defaults.
    #[serde(default)]
    pub enabled: bool,

    /// HTTP header name for the CSRF token. Default: `"X-CSRF-Token"`.
    #[serde(default = "default_csrf_header")]
    pub token_header: String,

    /// Form field name for the CSRF token. Default: `"_csrf"`.
    #[serde(default = "default_csrf_field")]
    pub form_field: String,

    /// Cookie name for storing the CSRF token. Default: `"autumn-csrf"`.
    #[serde(default = "default_csrf_cookie")]
    pub cookie_name: String,

    /// HTTP methods that do NOT require CSRF validation.
    /// Default: `["GET", "HEAD", "OPTIONS", "TRACE"]`.
    #[serde(default = "default_safe_methods")]
    pub safe_methods: Vec<String>,

    /// Request path prefixes that are exempt from CSRF validation.
    /// Default: `[]`.
    ///
    /// Use this to opt JSON API routes out of CSRF when they authenticate
    /// with bearer tokens or other non-cookie credentials. Matches are by
    /// prefix on the request path, e.g. `"/api/"` exempts all routes
    /// under `/api/`.
    #[serde(default)]
    pub exempt_paths: Vec<String>,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token_header: default_csrf_header(),
            form_field: default_csrf_field(),
            cookie_name: default_csrf_cookie(),
            safe_methods: default_safe_methods(),
            exempt_paths: Vec::new(),
        }
    }
}

/// Rate limiting configuration.
///
/// Applies a per-client-IP token bucket to every request. When a client
/// exceeds their bucket, the middleware returns `429 Too Many Requests`
/// with a `Retry-After` header indicating when to retry.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `false` |
/// | `requests_per_second` | `10.0` |
/// | `burst` | `20` |
/// | `trust_forwarded_headers` | `false` |
///
/// # Client IP resolution
///
/// By default the limiter keys on the **connection peer address**. This
/// prevents clients from bypassing throttling by rotating `X-Forwarded-For`
/// values. Set `trust_forwarded_headers = true` only when the server
/// sits behind a trusted reverse proxy that strips and rewrites
/// forwarding headers on every request.
///
/// # Examples
///
/// ```toml
/// [security.rate_limit]
/// enabled = true
/// requests_per_second = 5.0
/// burst = 10
/// trust_forwarded_headers = false
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting. Default: `false`.
    #[serde(default)]
    pub enabled: bool,

    /// Steady-state refill rate in requests per second. Default: `10.0`.
    #[serde(default = "default_rps")]
    pub requests_per_second: f64,

    /// Maximum burst capacity (number of tokens the bucket can hold).
    /// Default: `20`.
    #[serde(default = "default_burst")]
    pub burst: u32,

    /// Consult `X-Forwarded-For` / `X-Real-IP` before the connection peer
    /// when identifying the client. Default: `false`.
    ///
    /// Enable ONLY when the server is behind a trusted reverse proxy that
    /// fully overrides these headers on every request. Otherwise a client
    /// can rotate header values to bypass throttling.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            requests_per_second: default_rps(),
            burst: default_burst(),
            trust_forwarded_headers: false,
        }
    }
}

// ── Default value functions ────────────────────────────────────────

const fn default_true() -> bool {
    true
}

fn default_x_frame_options() -> String {
    "DENY".to_owned()
}

const fn default_hsts_max_age() -> u64 {
    31_536_000 // 1 year
}

fn default_referrer_policy() -> String {
    "strict-origin-when-cross-origin".to_owned()
}

/// Default `Content-Security-Policy` value.
///
/// Designed to be "sensible by default" while allowing htmx to function
/// normally when served from the same origin (as Autumn does for htmx and its
/// CSRF helper under `/static/js/`).
///
/// Directives:
/// - `default-src 'self'` -- everything defaults to same-origin
/// - `img-src 'self' data:` -- images from self and inline data URIs
/// - `style-src 'self' 'unsafe-inline'` -- same-origin stylesheets plus
///   inline `style` attributes (required by many UI libraries and
///   template engines)
/// - `script-src 'self'` -- only same-origin scripts; htmx and Autumn's htmx
///   CSRF helper work here because they are served from `/static/js/`
/// - `connect-src 'self'` -- `fetch`/`XHR`/htmx requests go to same origin
/// - `form-action 'self'` -- forms can only POST to same origin
/// - `frame-ancestors 'none'` -- matches the default `X-Frame-Options: DENY`
/// - `base-uri 'self'` -- prevents `<base>` hijacking
#[must_use]
pub fn default_content_security_policy() -> String {
    "default-src 'self'; \
     img-src 'self' data:; \
     style-src 'self' 'unsafe-inline'; \
     script-src 'self'; \
     connect-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     base-uri 'self'"
        .to_owned()
}

fn default_csrf_header() -> String {
    "X-CSRF-Token".to_owned()
}

fn default_csrf_field() -> String {
    "_csrf".to_owned()
}

fn default_csrf_cookie() -> String {
    "autumn-csrf".to_owned()
}

fn default_safe_methods() -> Vec<String> {
    vec![
        "GET".to_owned(),
        "HEAD".to_owned(),
        "OPTIONS".to_owned(),
        "TRACE".to_owned(),
    ]
}

const fn default_rps() -> f64 {
    10.0
}

const fn default_burst() -> u32 {
    20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_config_defaults() {
        let config = SecurityConfig::default();
        assert_eq!(config.headers.x_frame_options, "DENY");
        assert!(config.headers.x_content_type_options);
        assert!(config.headers.xss_protection);
        assert!(!config.headers.strict_transport_security);
        assert_eq!(config.headers.hsts_max_age_secs, 31_536_000);
        // Default CSP is non-empty and htmx-compatible.
        assert!(!config.headers.content_security_policy.is_empty());
        assert!(
            config
                .headers
                .content_security_policy
                .contains("default-src 'self'")
        );
        assert!(
            config
                .headers
                .content_security_policy
                .contains("script-src 'self'")
        );
        assert_eq!(
            config.headers.referrer_policy,
            "strict-origin-when-cross-origin"
        );
    }

    #[test]
    fn default_csp_does_not_allow_unsafe_eval() {
        // htmx works without unsafe-eval; only `hx-on` opts into it.
        // Keep the default tight so that the baseline policy passes
        // Mozilla Observatory and similar automated scanners.
        let csp = default_content_security_policy();
        assert!(!csp.contains("'unsafe-eval'"), "csp = {csp}");
        assert!(
            !csp.contains("'unsafe-inline' 'unsafe-eval'"),
            "csp = {csp}"
        );
    }

    #[test]
    fn csp_can_be_disabled_via_toml_empty_string() {
        let toml_str = r#"
            content_security_policy = ""
        "#;
        let config: HeadersConfig = toml::from_str(toml_str).unwrap();
        assert!(config.content_security_policy.is_empty());
    }

    #[test]
    fn csp_can_be_overridden_via_toml() {
        let toml_str = r#"
            content_security_policy = "default-src 'none'"
        "#;
        let config: HeadersConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.content_security_policy, "default-src 'none'");
    }

    #[test]
    fn csrf_config_defaults() {
        let config = CsrfConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.token_header, "X-CSRF-Token");
        assert_eq!(config.form_field, "_csrf");
        assert_eq!(config.cookie_name, "autumn-csrf");
        assert_eq!(config.safe_methods.len(), 4);
    }

    #[test]
    fn headers_config_deserialize() {
        let toml_str = r#"
            x_frame_options = "SAMEORIGIN"
            strict_transport_security = true
            content_security_policy = "default-src 'self'"
        "#;
        let config: HeadersConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.x_frame_options, "SAMEORIGIN");
        assert!(config.strict_transport_security);
        assert_eq!(config.content_security_policy, "default-src 'self'");
        // Defaults for unspecified fields
        assert!(config.x_content_type_options);
        assert!(config.xss_protection);
    }

    #[test]
    fn csrf_config_deserialize() {
        let toml_str = r#"
            enabled = true
            token_header = "X-XSRF-Token"
        "#;
        let config: CsrfConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.token_header, "X-XSRF-Token");
        assert_eq!(config.form_field, "_csrf"); // default preserved
    }

    #[test]
    fn rate_limit_config_defaults() {
        let config = RateLimitConfig::default();
        assert!(!config.enabled);
        assert!((config.requests_per_second - 10.0).abs() < f64::EPSILON);
        assert_eq!(config.burst, 20);
        assert!(!config.trust_forwarded_headers);
    }

    #[test]
    fn rate_limit_config_deserialize() {
        let toml_str = r"
            enabled = true
            requests_per_second = 5.0
            burst = 100
            trust_forwarded_headers = true
        ";
        let config: RateLimitConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert!((config.requests_per_second - 5.0).abs() < f64::EPSILON);
        assert_eq!(config.burst, 100);
        assert!(config.trust_forwarded_headers);
    }

    #[test]
    fn rate_limit_config_partial_deserialize_uses_defaults() {
        let toml_str = "enabled = true";
        let config: RateLimitConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert!((config.requests_per_second - 10.0).abs() < f64::EPSILON);
        assert_eq!(config.burst, 20);
        assert!(!config.trust_forwarded_headers);
    }

    #[test]
    fn full_security_config_deserialize() {
        let toml_str = r#"
            [headers]
            x_frame_options = "DENY"
            strict_transport_security = true

            [csrf]
            enabled = true

            [rate_limit]
            enabled = true
            requests_per_second = 50.0
            burst = 100
        "#;
        let config: SecurityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.headers.x_frame_options, "DENY");
        assert!(config.headers.strict_transport_security);
        assert!(config.csrf.enabled);
        assert!(config.rate_limit.enabled);
        assert!((config.rate_limit.requests_per_second - 50.0).abs() < f64::EPSILON);
        assert_eq!(config.rate_limit.burst, 100);
    }
}
