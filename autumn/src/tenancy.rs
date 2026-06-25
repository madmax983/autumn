use axum::{
    extract::State,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body::Body as HttpBody;
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// 1. Task-local storage for CURRENT_TENANT
tokio::task_local! {
    pub static CURRENT_TENANT: Option<String>;
}

// 2. Extractor structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant(pub String);

impl axum::extract::FromRequestParts<crate::AppState> for Tenant {
    type Rejection = crate::AutumnError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        // Fast path: when the tenancy middleware has already resolved and scoped
        // the tenant for this request, read it from the task-local rather than
        // performing a second extraction from headers/session/cookies.
        if let Ok(Some(tenant_id)) = CURRENT_TENANT.try_with(Clone::clone) {
            return Ok(Self(tenant_id));
        }

        let config = state
            .extension::<crate::config::AutumnConfig>()
            .ok_or_else(|| {
                crate::AutumnError::service_unavailable_msg("Config is not available")
            })?;
        let tenant_id = extract_tenant_from_parts(parts, &config).await?;
        Ok(Self(tenant_id))
    }
}

// Helper to run in-test tenancy contexts
pub async fn with_tenant<F, R>(tenant_id: String, future: F) -> R
where
    F: Future<Output = R>,
{
    CURRENT_TENANT.scope(Some(tenant_id), future).await
}

// Tenant extraction logic based on configuration
#[allow(clippy::missing_errors_doc, clippy::too_many_lines)]
pub async fn extract_tenant_from_parts(
    parts: &mut axum::http::request::Parts,
    config: &crate::config::AutumnConfig,
) -> Result<String, crate::AutumnError> {
    if !config.tenancy.enabled {
        return Err(crate::AutumnError::service_unavailable_msg(
            "Tenancy is not enabled; set [tenancy] enabled = true in autumn.toml",
        ));
    }

    match config.tenancy.source.as_str() {
        "header" => {
            let header_value = parts
                .headers
                .get(&config.tenancy.header_name)
                .ok_or_else(|| {
                    crate::AutumnError::bad_request_msg(format!(
                        "Missing required tenant header: {}",
                        config.tenancy.header_name
                    ))
                })?;
            let val = header_value
                .to_str()
                .map_err(|_| {
                    crate::AutumnError::bad_request_msg(format!(
                        "Invalid UTF-8 in tenant header: {}",
                        config.tenancy.header_name
                    ))
                })?
                .to_string();
            if val.trim().is_empty() {
                return Err(crate::AutumnError::bad_request_msg(format!(
                    "Tenant header {} is empty",
                    config.tenancy.header_name
                )));
            }
            Ok(val)
        }
        "subdomain" => {
            // Prefer the proxy-resolved host (honours X-Forwarded-Host from trusted
            // upstreams); fall back to the raw Host header when the layer has not run.
            let host_owned: String = parts
                .extensions
                .get::<crate::security::ResolvedClientIdentity>()
                .and_then(|id| id.host.clone())
                .map_or_else(
                    || {
                        parts
                            .headers
                            .get(axum::http::header::HOST)
                            .ok_or_else(|| {
                                crate::AutumnError::bad_request_msg(
                                    "Missing Host header for subdomain tenancy",
                                )
                            })
                            .and_then(|h| {
                                h.to_str().map(ToOwned::to_owned).map_err(|_| {
                                    crate::AutumnError::bad_request_msg(
                                        "Invalid UTF-8 in Host header",
                                    )
                                })
                            })
                    },
                    Ok,
                )?;

            let host = host_owned.as_str();
            let host_only = host.split(':').next().unwrap_or(host).trim();

            if host_only.parse::<std::net::IpAddr>().is_ok() {
                return Err(crate::AutumnError::bad_request_msg(
                    "IP address host not allowed in subdomain mode",
                ));
            }

            // DNS hostnames are case-insensitive; normalise to lowercase
            // before any matching so that e.g. `Tenant1.Example.COM` works.
            let host_lower = host_only.to_lowercase();

            if let Some(ref base_domain) = config.tenancy.base_domain {
                let base_domain_clean = base_domain.trim().to_lowercase();
                if !host_lower.ends_with(base_domain_clean.as_str()) {
                    return Err(crate::AutumnError::bad_request_msg(format!(
                        "Host does not match base domain: {base_domain_clean}"
                    )));
                }
                if host_lower.len() <= base_domain_clean.len() {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Apex domain not allowed in subdomain mode",
                    ));
                }
                let prefix_len = host_lower.len() - base_domain_clean.len();
                if !host_lower[..prefix_len].ends_with('.') {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Invalid subdomain format",
                    ));
                }
                let subdomain_part = &host_lower[..prefix_len - 1];
                let tenant = subdomain_part.split('.').next().ok_or_else(|| {
                    crate::AutumnError::bad_request_msg("Unable to extract subdomain tenant")
                })?;
                if tenant.trim().is_empty() {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Extracted subdomain tenant is empty",
                    ));
                }
                Ok(tenant.to_string())
            } else {
                let labels: Vec<&str> = host_lower.split('.').filter(|s| !s.is_empty()).collect();
                if labels.is_empty() {
                    return Err(crate::AutumnError::bad_request_msg("Empty host header"));
                }

                if labels.len() < 2 {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Apex or local host without subdomain not allowed",
                    ));
                }

                if labels.len() == 2 && labels[1] != "localhost" {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Apex domain not allowed in subdomain mode",
                    ));
                }

                let tenant = labels[0].to_string();
                if tenant.trim().is_empty() {
                    return Err(crate::AutumnError::bad_request_msg(
                        "Extracted subdomain tenant is empty",
                    ));
                }
                Ok(tenant)
            }
        }
        "session" => {
            let session = parts
                .extensions
                .get::<crate::session::Session>()
                .ok_or_else(|| {
                    crate::AutumnError::internal_server_error_msg(
                        "SessionLayer not installed but session tenancy source is configured",
                    )
                })?;
            let tenant = session
                .get(&config.tenancy.session_key)
                .await
                .ok_or_else(|| {
                    crate::AutumnError::unauthorized_msg(format!(
                        "Tenant ID missing from session key: {}",
                        config.tenancy.session_key
                    ))
                })?;
            if tenant.trim().is_empty() {
                return Err(crate::AutumnError::unauthorized_msg(format!(
                    "Tenant ID in session key {} is empty",
                    config.tenancy.session_key
                )));
            }
            Ok(tenant)
        }
        "jwt" => {
            let auth_header = parts
                .headers
                .get(axum::http::header::AUTHORIZATION)
                .ok_or_else(|| {
                    crate::AutumnError::unauthorized_msg(
                        "Missing Authorization header for JWT tenancy",
                    )
                })?;
            let auth_str = auth_header.to_str().map_err(|_| {
                crate::AutumnError::unauthorized_msg("Invalid UTF-8 in Authorization header")
            })?;

            if auth_str.len() < 7
                || !auth_str.is_char_boundary(7)
                || !auth_str[..7].eq_ignore_ascii_case("bearer ")
            {
                return Err(crate::AutumnError::unauthorized_msg(
                    "Invalid Authorization header format. Expected Bearer <token>",
                ));
            }
            let token = &auth_str[7..];

            let secret = config.tenancy.jwt_secret.as_ref().ok_or_else(|| {
                crate::AutumnError::unauthorized_msg("JWT secret is not configured")
            })?;

            let mut validation = ::jsonwebtoken::Validation::default();
            if let Some(ref iss) = config.tenancy.jwt_issuer {
                validation.set_issuer(::std::slice::from_ref(iss));
            }
            if let Some(ref aud) = config.tenancy.jwt_audience {
                validation.set_audience(&[aud.as_str()]);
            } else {
                validation.validate_aud = false;
            }

            let token_data = ::jsonwebtoken::decode::<serde_json::Value>(
                token,
                &::jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
                &validation,
            )
            .map_err(|e| {
                crate::AutumnError::unauthorized_msg(format!("JWT validation failed: {e}"))
            })?;

            // `jsonwebtoken`'s `set_audience` validates the `aud` value when
            // the claim is *present*, but silently accepts tokens that omit the
            // `aud` field entirely. Explicitly reject those when audience
            // validation is enabled so legacy tokens without an `aud` claim
            // cannot bypass the check.
            if let Some(ref expected_aud) = config.tenancy.jwt_audience {
                let aud_ok = token_data.claims.get("aud").is_some_and(|v| match v {
                    serde_json::Value::String(s) => s == expected_aud,
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .any(|e| e.as_str() == Some(expected_aud.as_str())),
                    _ => false,
                });
                if !aud_ok {
                    return Err(crate::AutumnError::unauthorized_msg(
                        "JWT audience validation failed: missing or invalid aud claim",
                    ));
                }
            }

            let tenant = token_data
                .claims
                .get(&config.tenancy.jwt_claim)
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::AutumnError::unauthorized_msg(format!(
                        "Tenant claim '{}' missing from JWT payload",
                        config.tenancy.jwt_claim
                    ))
                })?
                .to_string();

            if tenant.trim().is_empty() {
                return Err(crate::AutumnError::unauthorized_msg(format!(
                    "Tenant claim '{}' in JWT payload is empty",
                    config.tenancy.jwt_claim
                )));
            }
            Ok(tenant)
        }
        other => Err(crate::AutumnError::internal_server_error_msg(format!(
            "Unsupported tenancy source: {other}"
        ))),
    }
}

/// Returns true when `path` is exempt from tenant resolution.
///
/// A path is public if:
/// - it matches any entry in `tenancy.public_paths` (slash-boundary prefix; empty
///   entries and trailing slashes in the list are normalized away so they can't
///   accidentally exempt everything),
/// - it matches the configured health/liveness/readiness/startup probe paths,
/// - it is under the actuator prefix (e.g. `/actuator/prometheus`) — so Prometheus
///   scraping and ops tooling are never blocked by tenancy,
/// - it is the `OpenAPI` spec path (e.g. `/openapi.json`), or
/// - it exactly equals `tenancy.login_redirect` — so the redirect target itself is
///   always reachable even if the operator forgot to add it to `public_paths`,
///   preventing an infinite redirect loop.
///
/// Matching uses the same slash-boundary prefix semantics as the rest of the
/// framework (CSRF, CAPTCHA): `/login` matches `/login` and `/login/sso` but not
/// `/login-admin`.
fn is_public_path(path: &str, config: &crate::config::AutumnConfig) -> bool {
    // Guard against empty prefixes: `path_matches_route_prefix(path, "")` is true
    // for every absolute path, so an empty entry — whether a stray `public_paths`
    // item or a misconfigured built-in like `health.path = ""` — would otherwise
    // exempt the entire application from tenancy.
    let matches =
        |prefix: &str| !prefix.is_empty() && crate::router::path_matches_route_prefix(path, prefix);

    // User-configured public paths — normalize trailing slashes so `/static/`
    // behaves the same as `/static`, but preserve a bare `"/"` (a common landing
    // page) rather than trimming it away to an empty, never-matching string.
    let user_paths_match = config.tenancy.public_paths.iter().any(|p| {
        let p = if p.len() > 1 {
            p.trim_end_matches('/')
        } else {
            p.as_str()
        };
        matches(p)
    });

    // The login_redirect target must always be reachable to prevent an infinite
    // redirect loop when a user forgets to add it to public_paths. Compare against
    // the target's path component only: a target like `/login?next=/dashboard`
    // arrives back as `parts.uri.path() == "/login"`, so a raw string comparison
    // would miss it and re-loop.
    let redirect_match = config
        .tenancy
        .login_redirect
        .as_deref()
        .and_then(|target| target.split(['?', '#']).next())
        .is_some_and(|target_path| path == target_path);

    user_paths_match
        || redirect_match
        || matches(&config.health.path)
        || matches(&config.health.live_path)
        || matches(&config.health.ready_path)
        || matches(&config.health.startup_path)
        || matches(&config.actuator.prefix)
        // Only exempt the OpenAPI spec path when the docs endpoint is actually
        // mounted. With `enabled = false` the router never serves it, so a
        // tenant-scoped app that defines its own route at that path must still
        // go through tenant extraction rather than be silently exempted.
        || (config.openapi_runtime.enabled && matches(&config.openapi_runtime.path))
}

// Tenancy middleware for Axum requests
pub async fn tenancy_middleware(
    State(state): State<crate::AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(config) = state.extension::<crate::config::AutumnConfig>() else {
        return crate::AutumnError::internal_server_error_msg("AutumnConfig not found in AppState")
            .into_response();
    };

    if !config.tenancy.enabled {
        return next.run(request).await;
    }

    let (mut parts, body) = request.into_parts();

    // Public paths (login/signup pages, static assets, health probes) stay
    // reachable without a tenant so unauthenticated visitors can reach them —
    // otherwise a session/jwt-sourced SaaS could never show a login screen.
    if is_public_path(parts.uri.path(), &config) {
        return next.run(Request::from_parts(parts, body)).await;
    }

    let tenant_id = match extract_tenant_from_parts(&mut parts, &config).await {
        Ok(t) => t,
        Err(e) => {
            // For browser logins, bounce a missing/unauthenticated tenant to the
            // configured login page instead of returning a raw 401. Only do this
            // for clients that accept HTML (navigating browsers): API clients
            // (e.g. `Accept: application/json`) expect the 401 so their error
            // handling isn't broken by a 303 to a login page. Other error classes
            // (e.g. a 500 misconfiguration) are surfaced unchanged so real bugs
            // are not masked as login redirects.
            if e.status() == axum::http::StatusCode::UNAUTHORIZED
                && let Some(target) = &config.tenancy.login_redirect
                && parts
                    .headers
                    .get(axum::http::header::ACCEPT)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|accept| accept.contains("text/html"))
            {
                return axum::response::Redirect::to(target).into_response();
            }
            return e.into_response();
        }
    };

    // Tag the request-scoped log context (#1169) so every subsequent event
    // automatically carries the resolved tenant id.
    crate::log::context::set_tenant_id(&tenant_id);

    let request = Request::from_parts(parts, body);
    let tenant_id_clone = tenant_id.clone();
    let response = CURRENT_TENANT
        .scope(Some(tenant_id), next.run(request))
        .await;

    let (parts, body) = response.into_parts();
    let wrapped = TenantPropagatingBody {
        inner: body,
        tenant_id: tenant_id_clone,
    };
    Response::from_parts(parts, axum::body::Body::new(wrapped))
}

pin_project! {
    /// A response body wrapper that re-establishes the tenant context
    /// for each poll of the inner body, so lazy/streaming bodies can
    /// access tenant-scoped repositories during their polling phase.
    pub struct TenantPropagatingBody<B> {
        #[pin]
        pub inner: B,
        pub tenant_id: String,
    }
}

impl<B> HttpBody for TenantPropagatingBody<B>
where
    B: HttpBody,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let tenant_id = this.tenant_id.clone();
        CURRENT_TENANT.sync_scope(Some(tenant_id), || this.inner.poll_frame(cx))
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// A trait implemented by model insertable helper types to dynamically set tenant ID.
///
/// This sets or appends the tenant ID before database insertion. This avoids SQL duplicate
/// column errors when a model already has a manual (non-default) `tenant_id` field.
#[cfg(feature = "db")]
pub trait TenantInsertable<'a, Table> {
    type Values;
    fn tenant_values(self, tenant_id: &'a str) -> Self::Values;
}

/// Metadata about a model's `tenant_id` struct field.
#[cfg(feature = "db")]
pub trait ModelTenantIdMeta {
    /// True if the struct has a manual `tenant_id` field.
    const HAS_MANUAL_TENANT_ID: bool;
    /// Sets the tenant ID field on the struct if it has one.
    fn try_set_tenant_id(&mut self, tenant_id: &str);
}

/// A trait that bridges a Diesel table to its `tenant_id` column.
#[cfg(feature = "db")]
pub trait HasTenantIdColumn {
    type Column: ::diesel::Expression;
    fn column() -> Self::Column;
}

/// A selector helper to choose between different insertable values.
#[cfg(feature = "db")]
pub struct TenantInsertableValuesSelector<'a, T, Table, const HAS_MANUAL: bool> {
    pub inner: T,
    pub tenant_id: &'a str,
    pub _marker: std::marker::PhantomData<Table>,
}

/// A trait implemented by selector variants to get the actual insertable values.
#[cfg(feature = "db")]
pub trait GetInsertableValues {
    type Values;
    fn get_values(self) -> Self::Values;
}

#[cfg(feature = "db")]
impl<T, Table> GetInsertableValues for TenantInsertableValuesSelector<'_, T, Table, true>
where
    T: ModelTenantIdMeta,
{
    type Values = T;
    fn get_values(mut self) -> Self::Values {
        self.inner.try_set_tenant_id(self.tenant_id);
        self.inner
    }
}

#[cfg(feature = "db")]
impl<'a, T, Table> GetInsertableValues for TenantInsertableValuesSelector<'a, T, Table, false>
where
    Table: HasTenantIdColumn,
    Table::Column: ::diesel::ExpressionMethods,
    <Table::Column as ::diesel::Expression>::SqlType: ::diesel::sql_types::SqlType,
    &'a str: ::diesel::expression::AsExpression<<Table::Column as ::diesel::Expression>::SqlType>,
{
    type Values = (T, ::diesel::dsl::Eq<Table::Column, &'a str>);
    fn get_values(self) -> Self::Values {
        use ::diesel::ExpressionMethods;
        (self.inner, Table::column().eq(self.tenant_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::ResolvedClientIdentity;

    fn subdomain_config() -> crate::config::AutumnConfig {
        let mut c = crate::config::AutumnConfig::default();
        c.tenancy.enabled = true;
        c.tenancy.source = "subdomain".to_string();
        c
    }

    fn subdomain_config_with_base(base: &str) -> crate::config::AutumnConfig {
        let mut c = subdomain_config();
        c.tenancy.base_domain = Some(base.to_string());
        c
    }

    fn make_parts(host: &str) -> axum::http::request::Parts {
        let (parts, ()) = axum::http::Request::builder()
            .uri("http://ignored/")
            .header(axum::http::header::HOST, host)
            .body(())
            .unwrap()
            .into_parts();
        parts
    }

    fn make_parts_with_identity(
        host_header: &str,
        resolved_host: &str,
    ) -> axum::http::request::Parts {
        let (mut parts, ()) = axum::http::Request::builder()
            .uri("http://ignored/")
            .header(axum::http::header::HOST, host_header)
            .body(())
            .unwrap()
            .into_parts();
        parts.extensions.insert(ResolvedClientIdentity {
            addr: None,
            host: Some(resolved_host.to_string()),
            scheme: None,
        });
        parts
    }

    /// When no `ResolvedClientIdentity` extension is present, subdomain mode falls
    /// back to the raw Host header as before.
    #[tokio::test]
    async fn subdomain_falls_back_to_host_header_without_extension() {
        let config = subdomain_config();
        let mut parts = make_parts("tenant1.example.com");
        let result = extract_tenant_from_parts(&mut parts, &config).await;
        assert_eq!(result.unwrap(), "tenant1");
    }

    /// When `ResolvedClientIdentity.host` is present, subdomain mode uses it instead
    /// of the raw Host header so that X-Forwarded-Host from trusted proxies is honoured.
    #[tokio::test]
    async fn subdomain_uses_resolved_host_from_extension() {
        let config = subdomain_config();
        // Raw Host header is the internal address; resolved host is the public subdomain.
        let mut parts = make_parts_with_identity("internal.cluster.local", "tenant1.example.com");
        let result = extract_tenant_from_parts(&mut parts, &config).await;
        assert_eq!(result.unwrap(), "tenant1");
    }

    /// With a configured `base_domain`, the resolved host is matched against it.
    #[tokio::test]
    async fn subdomain_uses_resolved_host_with_base_domain() {
        let config = subdomain_config_with_base("example.com");
        let mut parts = make_parts_with_identity("internal.cluster.local", "acme.example.com");
        let result = extract_tenant_from_parts(&mut parts, &config).await;
        assert_eq!(result.unwrap(), "acme");
    }

    /// Port suffixes in the resolved host are stripped before subdomain extraction.
    #[tokio::test]
    async fn subdomain_strips_port_from_resolved_host() {
        let config = subdomain_config_with_base("example.com");
        let mut parts =
            make_parts_with_identity("internal.cluster.local", "tenant2.example.com:8080");
        let result = extract_tenant_from_parts(&mut parts, &config).await;
        assert_eq!(result.unwrap(), "tenant2");
    }

    /// When `ResolvedClientIdentity.host` is `None` (layer ran but found no host),
    /// subdomain mode falls back to the raw Host header.
    #[tokio::test]
    async fn subdomain_falls_back_when_resolved_host_is_none() {
        let config = subdomain_config();
        let (mut parts, ()) = axum::http::Request::builder()
            .uri("http://ignored/")
            .header(axum::http::header::HOST, "tenant3.example.com")
            .body(())
            .unwrap()
            .into_parts();
        parts.extensions.insert(ResolvedClientIdentity {
            addr: None,
            host: None,
            scheme: None,
        });
        let result = extract_tenant_from_parts(&mut parts, &config).await;
        assert_eq!(result.unwrap(), "tenant3");
    }

    fn public_paths_config(paths: &[&str]) -> crate::config::AutumnConfig {
        let mut c = crate::config::AutumnConfig::default();
        c.tenancy.public_paths = paths.iter().map(|s| (*s).to_string()).collect();
        c
    }

    /// A configured public path matches itself and any slash-delimited subpath.
    #[test]
    fn public_path_exact_and_subtree_match() {
        let c = public_paths_config(&["/login", "/static"]);
        assert!(is_public_path("/login", &c));
        assert!(is_public_path("/login/sso", &c));
        assert!(is_public_path("/static/css/app.css", &c));
    }

    /// Exemptions do not bleed into adjacent prefixes or unrelated routes.
    #[test]
    fn public_path_does_not_bleed_to_adjacent_prefix() {
        let c = public_paths_config(&["/login"]);
        assert!(!is_public_path("/login-admin", &c));
        assert!(!is_public_path("/dashboard", &c));
    }

    /// Health/liveness/readiness/startup probes are public without being listed.
    #[test]
    fn health_paths_are_always_public() {
        let c = crate::config::AutumnConfig::default();
        assert!(c.tenancy.public_paths.is_empty());
        assert!(is_public_path(&c.health.path, &c));
        assert!(is_public_path(&c.health.live_path, &c));
        assert!(is_public_path(&c.health.ready_path, &c));
        assert!(is_public_path(&c.health.startup_path, &c));
    }

    /// Empty-string entries in `public_paths` must not exempt every path.
    #[test]
    fn empty_public_path_entry_does_not_exempt_all() {
        let c = public_paths_config(&[""]);
        assert!(!is_public_path("/dashboard", &c));
        assert!(!is_public_path("/secret", &c));
    }

    /// Trailing-slash entries in `public_paths` behave like their unslashed form.
    #[test]
    fn trailing_slash_entry_matches_subtree() {
        let c = public_paths_config(&["/static/"]);
        assert!(is_public_path("/static", &c));
        assert!(is_public_path("/static/css/app.css", &c));
        assert!(!is_public_path("/dashboard", &c));
    }

    /// The actuator prefix is always public so Prometheus scraping is never blocked.
    #[test]
    fn actuator_prefix_is_always_public() {
        let c = crate::config::AutumnConfig::default();
        assert!(is_public_path(&c.actuator.prefix, &c));
        assert!(is_public_path(
            &format!("{}/prometheus", c.actuator.prefix),
            &c
        ));
        assert!(is_public_path(&format!("{}/health", c.actuator.prefix), &c));
    }

    /// The `OpenAPI` spec path is public while the docs endpoint is enabled.
    #[test]
    fn openapi_path_is_public_when_enabled() {
        let c = crate::config::AutumnConfig::default();
        assert!(c.openapi_runtime.enabled);
        assert!(is_public_path(&c.openapi_runtime.path, &c));
    }

    /// With the docs endpoint disabled, the configured spec path is NOT exempt —
    /// a tenant-scoped app may legitimately define its own route there.
    #[test]
    fn openapi_path_not_public_when_disabled() {
        let mut c = crate::config::AutumnConfig::default();
        c.openapi_runtime.enabled = false;
        assert!(!is_public_path(&c.openapi_runtime.path, &c));
    }

    /// The `login_redirect` target is always public even if missing from
    /// `public_paths`, preventing an infinite redirect loop.
    #[test]
    fn login_redirect_target_is_always_public() {
        let mut c = crate::config::AutumnConfig::default();
        c.tenancy.login_redirect = Some("/auth/login".to_string());
        // Not in public_paths — should still be reachable.
        assert!(is_public_path("/auth/login", &c));
        // Only the exact target, not adjacent paths.
        assert!(!is_public_path("/auth/login/sso", &c));
    }

    /// A `login_redirect` target carrying a query string is matched by its path
    /// component, so the login page is still exempted and no loop forms.
    #[test]
    fn login_redirect_target_with_query_is_public_by_path() {
        let mut c = crate::config::AutumnConfig::default();
        c.tenancy.login_redirect = Some("/login?next=/dashboard".to_string());
        // The follow-up request arrives as just the path.
        assert!(is_public_path("/login", &c));
        assert!(!is_public_path("/dashboard", &c));
    }

    /// A bare `"/"` in `public_paths` exempts the root exactly (a common landing
    /// page) and is not silently trimmed away to an empty, never-matching entry.
    #[test]
    fn root_public_path_is_preserved() {
        let c = public_paths_config(&["/"]);
        assert!(is_public_path("/", &c));
        // The slash-boundary semantics mean `/` matches only the root, not every
        // path, so protected routes still require a tenant.
        assert!(!is_public_path("/dashboard", &c));
    }

    /// A misconfigured empty built-in path (e.g. `health.path = ""`) must not
    /// exempt the whole application from tenancy.
    #[test]
    fn empty_builtin_path_does_not_exempt_all() {
        let mut c = crate::config::AutumnConfig::default();
        c.health.path = String::new();
        c.health.live_path = String::new();
        c.health.ready_path = String::new();
        c.health.startup_path = String::new();
        c.actuator.prefix = String::new();
        c.openapi_runtime.path = String::new();
        assert!(!is_public_path("/dashboard", &c));
        assert!(!is_public_path("/", &c));
    }
}
