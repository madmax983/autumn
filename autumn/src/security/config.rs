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
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecurityConfig {
    /// HTTP security headers applied to all responses.
    #[serde(default)]
    pub headers: HeadersConfig,

    /// CSRF (Cross-Site Request Forgery) protection.
    #[serde(default)]
    pub csrf: CsrfConfig,
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
    /// Default is an htmx-compatible policy that restricts resources to the
    /// same origin while permitting inline styles (common for htmx-driven
    /// UIs) and data-URI images. Set to `""` to disable the header entirely
    /// or override with a custom policy via `autumn.toml`.
    ///
    /// See [`default_content_security_policy`] for the exact default value.
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
///
/// # Examples
///
/// ```toml
/// [security.csrf]
/// enabled = true
/// token_header = "X-XSRF-Token"
/// cookie_name = "XSRF-TOKEN"
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
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token_header: default_csrf_header(),
            form_field: default_csrf_field(),
            cookie_name: default_csrf_cookie(),
            safe_methods: default_safe_methods(),
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

/// Default Content-Security-Policy used when no override is provided.
///
/// Designed to be strict enough to pass common security scanners while
/// still allowing htmx to function normally:
///
/// - `default-src 'self'` restricts all fetches to the same origin, covering
///   htmx's AJAX/SSE/WebSocket requests out of the box.
/// - `img-src 'self' data:` permits same-origin images and inline
///   `data:` images (common in htmx demos and dynamic content).
/// - `style-src 'self' 'unsafe-inline'` permits inline `style=""`
///   attributes, which are frequently used in htmx-rendered partials.
/// - `script-src 'self'` permits same-origin scripts only (htmx's
///   `hx-*` attributes are not affected by CSP because they are not
///   inline event handlers).
/// - `frame-ancestors 'none'` provides clickjacking protection even if
///   `X-Frame-Options` is absent.
/// - `base-uri 'self'` prevents `<base>` tag injection attacks.
/// - `form-action 'self'` restricts form submissions to the same origin.
#[must_use]
pub fn default_content_security_policy() -> String {
    "default-src 'self'; \
     img-src 'self' data:; \
     style-src 'self' 'unsafe-inline'; \
     script-src 'self'; \
     frame-ancestors 'none'; \
     base-uri 'self'; \
     form-action 'self'"
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
        // Default CSP is htmx-compatible (see S-049): same-origin default
        // with inline styles permitted so htmx-rendered partials work.
        let csp = &config.headers.content_security_policy;
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(
            config.headers.referrer_policy,
            "strict-origin-when-cross-origin"
        );
    }

    #[test]
    fn default_csp_allows_htmx() {
        // htmx relies on same-origin fetches (AJAX/SSE) and frequently
        // renders partials with inline style attributes. The default CSP
        // must permit both without requiring developer configuration.
        let csp = default_content_security_policy();
        assert!(
            csp.contains("default-src 'self'"),
            "default-src 'self' must cover htmx fetch/SSE/WebSocket requests"
        );
        assert!(
            csp.contains("style-src 'self' 'unsafe-inline'"),
            "inline styles must be permitted for htmx-rendered partials"
        );
        // unsafe-eval must NOT be present: htmx 2+ does not need it and
        // enabling it would weaken the default posture.
        assert!(
            !csp.contains("unsafe-eval"),
            "default CSP must not include 'unsafe-eval'"
        );
    }

    #[test]
    fn empty_csp_disables_header() {
        // Developers can disable the CSP entirely via autumn.toml.
        let toml_str = r#"
            content_security_policy = ""
        "#;
        let config: HeadersConfig = toml::from_str(toml_str).unwrap();
        assert!(config.content_security_policy.is_empty());
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
    fn full_security_config_deserialize() {
        let toml_str = r#"
            [headers]
            x_frame_options = "DENY"
            strict_transport_security = true

            [csrf]
            enabled = true
        "#;
        let config: SecurityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.headers.x_frame_options, "DENY");
        assert!(config.headers.strict_transport_security);
        assert!(config.csrf.enabled);
    }
}
