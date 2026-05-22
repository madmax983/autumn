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

// Internal base64url decoder without external dependencies
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while !s.len().is_multiple_of(4) {
        s.push('=');
    }
    let mut bytes = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let lookup = |c: char| -> Option<u8> {
        match c {
            'A'..='Z' => Some(c as u8 - b'A'),
            'a'..='z' => Some(c as u8 - b'a' + 26),
            '0'..='9' => Some(c as u8 - b'0' + 52),
            '+' => Some(62),
            '/' => Some(63),
            '=' => Some(0),
            _ => None,
        }
    };

    let mut i = 0;
    while i < chars.len() {
        if i + 3 >= chars.len() {
            break;
        }
        let c1 = lookup(chars[i])?;
        let c2 = lookup(chars[i + 1])?;
        let c3 = lookup(chars[i + 2])?;
        let c4 = lookup(chars[i + 3])?;

        let b1 = (c1 << 2) | (c2 >> 4);
        let b2 = ((c2 & 0x0f) << 4) | (c3 >> 2);
        let b3 = ((c3 & 0x03) << 6) | c4;

        bytes.push(b1);
        if chars[i + 2] != '=' {
            bytes.push(b2);
        }
        if chars[i + 3] != '=' {
            bytes.push(b3);
        }
        i += 4;
    }
    Some(bytes)
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
            let subdomain = host.split('.').next().ok_or_else(|| {
                crate::AutumnError::bad_request_msg("Unable to extract subdomain from Host header")
            })?;
            let tenant = subdomain.split(':').next().unwrap_or(subdomain).to_string();
            if tenant.trim().is_empty() {
                return Err(crate::AutumnError::bad_request_msg(
                    "Extracted subdomain tenant is empty",
                ));
            }
            Ok(tenant)
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
            if !auth_str.starts_with("Bearer ") {
                return Err(crate::AutumnError::unauthorized_msg(
                    "Invalid Authorization header format. Expected Bearer <token>",
                ));
            }
            let token = &auth_str[7..];
            let payload = token
                .split('.')
                .nth(1)
                .ok_or_else(|| crate::AutumnError::unauthorized_msg("Invalid JWT token format"))?;
            let decoded_bytes = base64url_decode(payload).ok_or_else(|| {
                crate::AutumnError::unauthorized_msg("Failed to decode JWT payload base64url")
            })?;
            let json: serde_json::Value = serde_json::from_slice(&decoded_bytes).map_err(|_| {
                crate::AutumnError::unauthorized_msg("Failed to parse JWT payload as JSON")
            })?;
            let tenant = json
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
        return crate::AutumnError::internal_server_error_msg(
            "AutumnConfig not found in AppState",
        )
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
