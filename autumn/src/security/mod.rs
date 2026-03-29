//! Spring Security-style protection for Autumn applications.
//!
//! This module provides automatic security hardening that follows OWASP
//! best practices. Like Spring Security, it applies sensible defaults
//! out of the box and can be customized via `autumn.toml`.
//!
//! ## What's included
//!
//! | Component | Module | Description |
//! |-----------|--------|-------------|
//! | Security headers | [`headers`] | X-Frame-Options, X-Content-Type-Options, HSTS, CSP, etc. |
//! | CSRF protection | [`csrf`] | Token-based CSRF validation for mutating requests |
//! | Configuration | [`config`] | `[security]` section in `autumn.toml` |
//!
//! Authentication, session management, and password hashing live in
//! their own top-level modules ([`crate::auth`], [`crate::session`]).
//!
//! ## Profile-aware defaults
//!
//! Like Spring Security's auto-configuration, Autumn adjusts security
//! settings based on the active profile:
//!
//! | Setting | `dev` | `prod` |
//! |---------|-------|--------|
//! | Security headers | Applied (all defaults) | Applied (all defaults + HSTS) |
//! | CSRF protection | Disabled | Enabled |
//! | HSTS | Off | On (1 year, includeSubDomains) |
//! | Session cookies | Not Secure | Secure |
//!
//! ## Configuration
//!
//! ```toml
//! [security.headers]
//! x_frame_options = "DENY"            # or "SAMEORIGIN", "" to disable
//! x_content_type_options = true        # X-Content-Type-Options: nosniff
//! xss_protection = true                # X-XSS-Protection: 1; mode=block
//! strict_transport_security = true     # HSTS (auto-enabled in prod)
//! hsts_max_age_secs = 31536000         # 1 year
//! content_security_policy = ""         # set to enable CSP
//! referrer_policy = "strict-origin-when-cross-origin"
//! permissions_policy = ""              # set to enable Permissions-Policy
//!
//! [security.csrf]
//! enabled = true                       # auto-enabled in prod
//! token_header = "X-CSRF-Token"
//! cookie_name = "autumn-csrf"
//! ```
//!
//! ## Quick start
//!
//! Security headers are applied automatically -- no setup needed.
//! For CSRF protection in templates:
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::security::CsrfToken;
//!
//! #[get("/form")]
//! async fn form(csrf: CsrfToken) -> Markup {
//!     html! {
//!         form method="POST" action="/submit" {
//!             input type="hidden" name="_csrf" value=(csrf.token());
//!             input type="text" name="title";
//!             button { "Submit" }
//!         }
//!     }
//! }
//! ```

pub mod config;
pub mod csrf;
pub mod headers;

// Re-export commonly used types at the module level.
pub use config::{CsrfConfig, HeadersConfig, SecurityConfig};
pub use csrf::{CsrfLayer, CsrfToken};
pub use headers::SecurityHeadersLayer;
