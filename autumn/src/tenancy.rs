use axum::{
    extract::State,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::future::Future;

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
        return Err(crate::AutumnError::bad_request_msg("Tenancy is disabled"));
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
            let host_header = parts.headers.get(axum::http::header::HOST).ok_or_else(|| {
                crate::AutumnError::bad_request_msg("Missing Host header for subdomain tenancy")
            })?;
            let host = host_header
                .to_str()
                .map_err(|_| crate::AutumnError::bad_request_msg("Invalid UTF-8 in Host header"))?;

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

            if auth_str.len() < 7 || !auth_str[..7].eq_ignore_ascii_case("bearer ") {
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
                let aud_ok = token_data
                    .claims
                    .get("aud")
                    .is_some_and(|v| match v {
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
    let tenant_id = match extract_tenant_from_parts(&mut parts, &config).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };

    let request = Request::from_parts(parts, body);
    CURRENT_TENANT
        .scope(Some(tenant_id), next.run(request))
        .await
}
