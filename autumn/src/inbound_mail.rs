//! First-class inbound email handling with provider webhooks and routing DSL.
//!
//! Three provider adapters ship in-tree, all available when the `inbound-mail`
//! Cargo feature is enabled:
//! - **Mailgun** — HTTP webhook with HMAC-SHA256 signature (also enable `inbound-mailgun`).
//! - **SES via SNS** — AWS SNS notification with subscription confirmation
//!   (also enable `inbound-ses`).
//! - **Generic** — raw RFC 5322 body for Postfix/relay setups.
//!
//! The `inbound-mailgun` and `inbound-ses` features are additive aliases that
//! imply `inbound-mail`. They exist to signal provider intent in `Cargo.toml`
//! and to allow future provider-specific optional dependencies.
//!
//! # Quick start
//!
//! ```toml
//! # Cargo.toml
//! autumn-web = { version = "...", features = ["inbound-mailgun"] }
//! ```
//!
//! ```rust,ignore
//! use autumn_web::inbound_mail::{InboundEmail, InboundMailEndpointConfig,
//!     InboundMailHandlerInfo, InboundMailRouter, ProcessingMode, RecipientPattern};
//!
//! fn handle_support(email: InboundEmail)
//!     -> std::pin::Pin<Box<dyn std::future::Future<Output = autumn_web::AutumnResult<()>>
//!        + Send + 'static>>
//! {
//!     Box::pin(async move {
//!         tracing::info!(from = %email.from, subject = %email.subject, "inbound support email");
//!         Ok(())
//!     })
//! }
//!
//! autumn_web::app()
//!     .inbound_mail_router(
//!         InboundMailRouter::new()
//!             .endpoint(InboundMailEndpointConfig::mailgun("/inbound/mailgun", "signing-key"))
//!             .handler(InboundMailHandlerInfo {
//!                 name: "support",
//!                 pattern: RecipientPattern::Exact("support@company.com".to_string()),
//!                 processing: ProcessingMode::Background,
//!                 handler: handle_support,
//!             })
//!     )
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use serde::Deserialize;

// ── SNS signature verification ────────────────────────────────────────────────

#[cfg(feature = "inbound-ses")]
mod sns_verify {
    use super::*;

    /// Validate that a `SigningCertURL` comes from the AWS SNS service.
    ///
    /// Accepted form: `https://sns.<region>.amazonaws.com/…` (or `.cn`).
    pub(super) fn is_valid_sns_cert_url(url: &str) -> bool {
        let Some(host) = url.strip_prefix("https://") else {
            return false;
        };
        let host = host.split('/').next().unwrap_or("");
        // Must be sns.<anything>.amazonaws.com or sns.<anything>.amazonaws.com.cn
        let base = host.strip_suffix(".cn").unwrap_or(host);
        base.ends_with(".amazonaws.com") && base.starts_with("sns.") && !host.contains("..")
    }

    /// Build the canonical string SNS uses for signing.
    pub(super) fn canonical_string(json: &serde_json::Value, msg_type: &str) -> Option<String> {
        let mut s = String::new();
        let fields: &[&str] = match msg_type {
            "Notification" => &[
                "Message",
                "MessageId",
                "Subject",
                "Timestamp",
                "TopicArn",
                "Type",
            ],
            "SubscriptionConfirmation" | "UnsubscribeConfirmation" => &[
                "Message",
                "MessageId",
                "SubscribeURL",
                "Timestamp",
                "Token",
                "TopicArn",
                "Type",
            ],
            _ => return None,
        };
        for field in fields {
            if let Some(v) = json.get(field).and_then(|v| v.as_str()) {
                s.push_str(field);
                s.push('\n');
                s.push_str(v);
                s.push('\n');
            }
        }
        Some(s)
    }

    // ── Minimal DER parser (X.509 SubjectPublicKeyInfo extraction) ────────────

    fn read_der_length(data: &[u8], pos: &mut usize) -> Option<usize> {
        let first = *data.get(*pos)?;
        *pos += 1;
        if first & 0x80 == 0 {
            return Some(first as usize);
        }
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for _ in 0..n {
            len = (len << 8) | (*data.get(*pos)? as usize);
            *pos += 1;
        }
        Some(len)
    }

    fn skip_tlv(data: &[u8], pos: &mut usize) -> Option<()> {
        *pos += 1; // tag
        let len = read_der_length(data, pos)?;
        *pos = pos.checked_add(len).filter(|&e| e <= data.len())?;
        Some(())
    }

    fn enter_sequence(data: &[u8], pos: &mut usize) -> Option<(usize, usize)> {
        if *data.get(*pos)? != 0x30 {
            return None;
        }
        *pos += 1;
        let len = read_der_length(data, pos)?;
        let content_start = *pos;
        *pos = content_start
            .checked_add(len)
            .filter(|&e| e <= data.len())?;
        Some((content_start, len))
    }

    /// Extract the DER-encoded SubjectPublicKeyInfo from a DER X.509 certificate.
    pub(super) fn extract_spki_from_der(cert_der: &[u8]) -> Option<Vec<u8>> {
        let mut pos = 0;
        // Certificate SEQUENCE
        let (_, _) = enter_sequence(cert_der, &mut pos)?;
        // Restart pos at TBSCertificate
        let mut pos = 0;
        skip_tlv(cert_der, &mut pos)?; // skip outer tag+len, get to TBS content
        // Re-enter properly: outer Certificate SEQUENCE → TBSCertificate SEQUENCE
        let mut outer = 0usize;
        let (cert_start, cert_len) = enter_sequence(cert_der, &mut outer)?;
        let tbs_data = &cert_der[cert_start..cert_start + cert_len];

        let mut tbs = 0usize;
        let (tbs_start, tbs_len) = enter_sequence(tbs_data, &mut tbs)?;
        let tbs_content = &tbs_data[tbs_start..tbs_start + tbs_len];

        let mut p = 0usize;
        // Skip version [0] EXPLICIT (tag 0xa0) if present.
        if tbs_content.get(p) == Some(&0xa0) {
            skip_tlv(tbs_content, &mut p)?;
        }
        skip_tlv(tbs_content, &mut p)?; // serialNumber
        skip_tlv(tbs_content, &mut p)?; // signature AlgorithmIdentifier
        skip_tlv(tbs_content, &mut p)?; // issuer
        skip_tlv(tbs_content, &mut p)?; // validity
        skip_tlv(tbs_content, &mut p)?; // subject

        // subjectPublicKeyInfo starts here; capture the full TLV.
        let spki_start = p;
        skip_tlv(tbs_content, &mut p)?;
        Some(tbs_content[spki_start..p].to_vec())
    }

    /// Decode a PEM certificate (first CERTIFICATE block) to DER bytes.
    pub(super) fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
        let start = pem.find("-----BEGIN CERTIFICATE-----")?;
        let after = &pem[start + "-----BEGIN CERTIFICATE-----".len()..];
        let end = after.find("-----END CERTIFICATE-----")?;
        let b64: String = after[..end]
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD.decode(b64).ok()
    }

    /// Verify an RSA PKCS#1 v1.5 signature with SHA-1 or SHA-256.
    pub(super) fn verify_rsa_signature(
        spki_der: &[u8],
        message: &[u8],
        sig: &[u8],
        sig_version: &str,
    ) -> Result<(), ()> {
        use rsa::pkcs1v15::Pkcs1v15Sign;
        use rsa::pkcs8::DecodePublicKey as _;
        use sha2::Digest as _;

        let public_key = rsa::RsaPublicKey::from_public_key_der(spki_der).map_err(|_| ())?;
        match sig_version {
            "2" => {
                let hash = sha2::Sha256::digest(message);
                public_key
                    .verify(Pkcs1v15Sign::new::<sha2::Sha256>(), &hash, sig)
                    .map_err(|_| ())
            }
            _ => {
                // SignatureVersion 1 uses SHA-1.
                // sha1 0.10 uses const-oid 0.10.x while rsa 0.9 uses const-oid 0.9.x,
                // so Pkcs1v15Sign::new::<sha1::Sha1>() fails to compile.  Work around
                // the version split by using new_unprefixed() and prepending the
                // DigestInfo DER structure (RFC 3447 §9.2) manually.
                use sha1::Digest as _;
                let hash = sha1::Sha1::digest(message);
                // SHA-1 DigestInfo prefix: SEQUENCE { SEQUENCE { OID sha1, NULL }, OCTET STRING }
                const SHA1_DI_PREFIX: &[u8] = &[
                    0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00,
                    0x04, 0x14,
                ];
                let mut digest_info = SHA1_DI_PREFIX.to_vec();
                digest_info.extend_from_slice(&hash);
                public_key
                    .verify(Pkcs1v15Sign::new_unprefixed(), &digest_info, sig)
                    .map_err(|_| ())
            }
        }
    }

    /// Verify the SNS notification signature.
    ///
    /// Set `AUTUMN_SES_SKIP_SNS_VERIFICATION=1` to disable in tests/local dev.
    pub(super) async fn verify(
        json: &serde_json::Value,
        http_client: &reqwest::Client,
    ) -> Result<(), StatusCode> {
        if std::env::var("AUTUMN_SES_SKIP_SNS_VERIFICATION").is_ok() {
            return Ok(());
        }
        let msg_type = json.get("Type").and_then(|v| v.as_str()).unwrap_or("");
        let cert_url = json
            .get("SigningCertURL")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let sig_b64 = json.get("Signature").and_then(|v| v.as_str()).unwrap_or("");
        let sig_version = json
            .get("SignatureVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("1");

        if !is_valid_sns_cert_url(cert_url) {
            tracing::warn!(cert_url, "inbound_mail.ses: invalid SNS SigningCertURL");
            return Err(StatusCode::UNAUTHORIZED);
        }
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(sig_b64)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let canonical = canonical_string(json, msg_type).ok_or(StatusCode::BAD_REQUEST)?;

        let cert_pem = http_client
            .get(cert_url)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "inbound_mail.ses: failed to fetch SNS cert");
                StatusCode::BAD_GATEWAY
            })?
            .text()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        let cert_der = pem_to_der(&cert_pem).ok_or_else(|| {
            tracing::error!("inbound_mail.ses: could not parse SNS signing cert PEM");
            StatusCode::UNAUTHORIZED
        })?;
        let spki = extract_spki_from_der(&cert_der).ok_or_else(|| {
            tracing::error!("inbound_mail.ses: could not extract public key from SNS cert");
            StatusCode::UNAUTHORIZED
        })?;
        verify_rsa_signature(&spki, canonical.as_bytes(), &sig_bytes, sig_version).map_err(|_| {
            tracing::warn!("inbound_mail.ses: SNS signature verification failed");
            StatusCode::UNAUTHORIZED
        })
    }
}

// ── Signature helpers ─────────────────────────────────────────────────────────

/// Compute the Mailgun HMAC-SHA256 webhook signature.
///
/// Mailgun signs webhooks as `HMAC-SHA256(timestamp || token, signing_key)`.
/// Returns a lowercase hex string (64 characters).
///
/// # Panics
///
/// Panics if `signing_key` cannot be used to initialize the HMAC (which the
/// underlying library guarantees never happens for any key size).
#[must_use]
pub fn compute_mailgun_signature(timestamp: &str, token: &str, signing_key: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(signing_key.as_bytes()).expect("HMAC can accept any key size");
    mac.update(timestamp.as_bytes());
    mac.update(token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

// ── Core email type ───────────────────────────────────────────────────────────

/// A parsed inbound email message received via webhook or relay.
#[derive(Debug, Clone)]
pub struct InboundEmail {
    /// Sender address (From header).
    pub from: String,
    /// To recipients.
    pub to: Vec<String>,
    /// CC recipients.
    pub cc: Vec<String>,
    /// Message subject.
    pub subject: String,
    /// Plain-text body, if present.
    pub text_body: Option<String>,
    /// HTML body, if present.
    pub html_body: Option<String>,
    /// All message headers keyed by lower-cased name.
    pub headers: HashMap<String, String>,
    /// Parsed attachments.
    pub attachments: Vec<Attachment>,
    /// Provider spam verdict, when reported.
    pub spam_report: Option<SpamReport>,
    /// Raw RFC 5322 bytes of the original message.
    pub raw: Bytes,
    /// Plus-address tag captured by the routing DSL (e.g. `ticket-42` from
    /// `replies+ticket-42@app.example`).
    pub plus_token: Option<String>,
}

impl InboundEmail {
    /// Return the captured plus-address tag, if any.
    #[must_use]
    pub fn plus_token(&self) -> Option<&str> {
        self.plus_token.as_deref()
    }

    /// Return the primary To address, if any.
    #[must_use]
    pub fn primary_recipient(&self) -> Option<&str> {
        self.to.first().map(String::as_str)
    }
}

/// An email attachment.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Filename from Content-Disposition, if present.
    pub filename: Option<String>,
    /// MIME content type.
    pub content_type: String,
    /// Raw attachment bytes.
    pub data: Bytes,
}

/// Provider spam report.
#[derive(Debug, Clone)]
pub struct SpamReport {
    /// Numeric spam score (higher = more likely spam).
    pub score: Option<f64>,
    /// Provider verdict string (e.g. `"Yes"`, `"No"`, `"Neutral"`).
    pub verdict: Option<String>,
}

// ── Provider enum ─────────────────────────────────────────────────────────────

/// The webhook provider that sends inbound mail to this endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboundMailProvider {
    /// Mailgun HTTP webhook with HMAC-SHA256 signature verification.
    Mailgun,
    /// AWS SES delivered via SNS notification.
    Ses,
    /// Generic RFC 5322 raw-body endpoint (no provider-specific signature).
    #[default]
    Generic,
}

// ── Processing mode ───────────────────────────────────────────────────────────

/// Controls when the webhook returns 200 relative to handler execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessingMode {
    /// Wait for the handler to complete before responding 200.
    Sync,
    /// Return 200 immediately; run the handler in a background Tokio task.
    #[default]
    Background,
}

// ── Endpoint configuration ────────────────────────────────────────────────────

/// Configuration for one inbound mail HTTP endpoint.
///
/// Each endpoint configuration creates one POST route (e.g.
/// `POST /inbound/mailgun`) that verifies the provider signature and
/// dispatches the parsed email to the registered handlers.
#[derive(Debug, Clone)]
pub struct InboundMailEndpointConfig {
    /// Route path (e.g. `"/inbound/mailgun"`).
    pub path: String,
    /// Which provider sends to this endpoint.
    pub provider: InboundMailProvider,
    /// Current signing key (literal value).
    pub signing_key: Option<String>,
    /// Environment variable that holds the signing key at startup.
    pub signing_key_env: Option<String>,
    /// Default processing mode for all handlers on this endpoint.
    ///
    /// Note: this field is reserved for future use. Dispatch currently
    /// honours each [`InboundMailHandlerInfo::processing`] value directly;
    /// the endpoint-level default is not yet applied automatically.
    pub processing: ProcessingMode,
}

impl Default for InboundMailEndpointConfig {
    fn default() -> Self {
        Self {
            path: "/inbound/mail".to_string(),
            provider: InboundMailProvider::Generic,
            signing_key: None,
            signing_key_env: None,
            processing: ProcessingMode::Background,
        }
    }
}

impl InboundMailEndpointConfig {
    /// Mailgun webhook endpoint with an explicit signing key.
    #[must_use]
    pub fn mailgun(path: impl Into<String>, signing_key: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            provider: InboundMailProvider::Mailgun,
            signing_key: Some(signing_key.into()),
            signing_key_env: None,
            processing: ProcessingMode::Background,
        }
    }

    /// AWS SES via SNS endpoint.
    ///
    /// No signing key is configured here: SNS subscription confirmation is
    /// handled automatically, and SNS message authenticity is verified via
    /// the `X-Amz-Sns-Message-Type` header.
    #[must_use]
    pub fn ses(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            provider: InboundMailProvider::Ses,
            signing_key: None,
            signing_key_env: None,
            processing: ProcessingMode::Background,
        }
    }

    /// Generic RFC 5322 raw-body endpoint with optional HMAC signing.
    ///
    /// When `signing_key` / `signing_key_env` is set, the handler verifies
    /// `X-Inbound-Signature: HMAC-SHA256(key, body)` before parsing.
    #[must_use]
    pub fn generic(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            provider: InboundMailProvider::Generic,
            signing_key: None,
            signing_key_env: None,
            processing: ProcessingMode::Background,
        }
    }

    /// Resolve the signing key: literal value first, then env var.
    #[must_use]
    pub fn resolve_signing_key(&self) -> Option<String> {
        if self.signing_key.is_some() {
            return self.signing_key.clone();
        }
        self.signing_key_env
            .as_deref()
            .and_then(|var| std::env::var(var).ok())
    }
}

// ── Routing DSL ───────────────────────────────────────────────────────────────

/// Recipient address matching rule.
///
/// Handlers are tried in registration order; the first matching pattern wins.
#[derive(Debug, Clone)]
pub enum RecipientPattern {
    /// Exact address match (case-insensitive, e.g. `"support@company.com"`).
    Exact(String),
    /// Local-part prefix match.
    ///
    /// Matches any address whose local part (before `@`) starts with the given
    /// prefix. `"ticket"` matches `"ticket@example.com"` and
    /// `"ticket+123@example.com"`.
    LocalPrefix(String),
    /// Plus-address routing: `"{local}+{token}@{domain}"`.
    ///
    /// The captured `{token}` is available via [`InboundEmail::plus_token()`].
    /// When `domain` is `None` any domain matches.
    PlusAddress {
        /// Local part before the `+` (e.g. `"replies"`).
        local: String,
        /// Optional domain restriction (e.g. `"app.example"`).
        domain: Option<String>,
    },
    /// Matches every incoming message unconditionally.
    Any,
}

impl RecipientPattern {
    /// Return `true` if this pattern matches `address`.
    #[must_use]
    pub fn matches(&self, address: &str) -> bool {
        let lower = address.to_ascii_lowercase();
        match self {
            Self::Exact(expected) => lower == expected.to_ascii_lowercase(),
            Self::LocalPrefix(prefix) => {
                local_part(&lower).starts_with(&prefix.to_ascii_lowercase() as &str)
            }
            Self::PlusAddress { local, domain } => {
                let Some((addr_local, addr_domain)) = split_address(&lower) else {
                    return false;
                };
                let prefix = format!("{}+", local.to_ascii_lowercase());
                if !addr_local.starts_with(&prefix) {
                    return false;
                }
                domain
                    .as_ref()
                    .is_none_or(|dom| addr_domain == dom.to_ascii_lowercase())
            }
            Self::Any => true,
        }
    }

    /// Extract the plus-address token from `address` if this is a
    /// [`PlusAddress`](Self::PlusAddress) pattern and the address matches.
    ///
    /// The returned token preserves its original casing.
    #[must_use]
    pub fn extract_token(&self, address: &str) -> Option<String> {
        let Self::PlusAddress { local, domain } = self else {
            return None;
        };
        let lower = address.to_ascii_lowercase();
        let (addr_local_lower, addr_domain) = split_address(&lower)?;
        if let Some(dom) = domain
            && addr_domain != dom.to_ascii_lowercase()
        {
            return None;
        }
        let prefix = format!("{}+", local.to_ascii_lowercase());
        if !addr_local_lower.starts_with(&prefix) {
            return None;
        }
        // Extract from the original address so the token's casing is preserved.
        let (orig_local, _) = split_address(address)?;
        orig_local.get(prefix.len()..).map(str::to_string)
    }
}

fn split_address(addr: &str) -> Option<(&str, &str)> {
    let at = addr.rfind('@')?;
    Some((&addr[..at], &addr[at + 1..]))
}

fn local_part(addr: &str) -> &str {
    addr.rfind('@').map_or(addr, |at| &addr[..at])
}

// ── Handler types ─────────────────────────────────────────────────────────────

/// Async inbound mail handler function.
pub type InboundMailHandlerFn =
    fn(InboundEmail) -> Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send + 'static>>;

/// A registered inbound mail handler with its routing metadata.
pub struct InboundMailHandlerInfo {
    /// Unique handler name used in logs and diagnostics.
    pub name: &'static str,
    /// Recipient pattern this handler matches.
    pub pattern: RecipientPattern,
    /// Whether to await the handler (Sync) or spawn it (Background).
    pub processing: ProcessingMode,
    /// The async handler function.
    pub handler: InboundMailHandlerFn,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Inbound mail router that wires webhook endpoints to handler functions.
///
/// Create with [`InboundMailRouter::new()`], chain builder methods, then pass
/// to [`AppBuilder::inbound_mail_router`](crate::app::AppBuilder::inbound_mail_router).
pub struct InboundMailRouter {
    pub(crate) endpoints: Vec<InboundMailEndpointConfig>,
    pub(crate) handlers: Vec<InboundMailHandlerInfo>,
    pub(crate) fallback: Option<InboundMailHandlerFn>,
    pub(crate) bounce_handler: Option<InboundMailHandlerFn>,
    pub(crate) complaint_handler: Option<InboundMailHandlerFn>,
}

impl Default for InboundMailRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl InboundMailRouter {
    /// Create a new, empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            handlers: Vec::new(),
            fallback: None,
            bounce_handler: None,
            complaint_handler: None,
        }
    }

    /// Add a provider endpoint. One HTTP POST route is created per call.
    #[must_use]
    pub fn endpoint(mut self, config: InboundMailEndpointConfig) -> Self {
        self.endpoints.push(config);
        self
    }

    /// Register a handler. Evaluated in registration order.
    #[must_use]
    pub fn handler(mut self, info: InboundMailHandlerInfo) -> Self {
        self.handlers.push(info);
        self
    }

    /// Register a catch-all fallback for messages that no handler matches.
    #[must_use]
    pub fn fallback(mut self, f: InboundMailHandlerFn) -> Self {
        self.fallback = Some(f);
        self
    }

    /// Register a handler for bounce events.
    ///
    /// Bounce detection is provider-specific. For Mailgun, the presence of
    /// an `X-Mailgun-Bounced-Address` form field signals a bounce event.
    #[must_use]
    pub fn on_bounce(mut self, f: InboundMailHandlerFn) -> Self {
        self.bounce_handler = Some(f);
        self
    }

    /// Register a handler for complaint events.
    #[must_use]
    pub fn on_complaint(mut self, f: InboundMailHandlerFn) -> Self {
        self.complaint_handler = Some(f);
        self
    }

    /// Dispatch a parsed email to the first matching handler.
    ///
    /// Evaluation order:
    /// 1. Bounce handler (when `x-mailgun-bounced-address` header present).
    /// 2. Complaint handler (when `x-mailgun-spam-flag: YES` header present).
    /// 3. Registered handlers, in order.
    /// 4. Fallback handler, if registered.
    /// 5. Log + drop with a `WARN` trace.
    pub(crate) async fn dispatch(&self, mut email: InboundEmail) -> crate::AutumnResult<()> {
        // Bounce detection via Mailgun header.
        if email.headers.contains_key("x-mailgun-bounced-address")
            && let Some(handler) = self.bounce_handler
        {
            return handler(email).await;
        }

        for info in &self.handlers {
            // Any pattern must fire even when email.to is empty (e.g. Bcc-only delivery).
            let matched = if matches!(info.pattern, RecipientPattern::Any) {
                Some(email.to.first().cloned().unwrap_or_default())
            } else {
                email.to.iter().find(|r| info.pattern.matches(r)).cloned()
            };
            if let Some(recipient) = matched {
                if let Some(token) = info.pattern.extract_token(&recipient) {
                    email.plus_token = Some(token);
                }
                match info.processing {
                    ProcessingMode::Sync => return (info.handler)(email).await,
                    ProcessingMode::Background => {
                        let fut = (info.handler)(email);
                        tokio::spawn(async move {
                            if let Err(e) = fut.await {
                                tracing::error!(error = %e, "inbound_mail: background handler error");
                            }
                        });
                        return Ok(());
                    }
                }
            }
        }

        if let Some(fallback) = self.fallback {
            return fallback(email).await;
        }

        tracing::warn!(
            from = %email.from,
            to = ?email.to,
            subject = %email.subject,
            "inbound_mail.unmatched: no handler matched; message dropped"
        );
        Ok(())
    }
}

// ── Provider parsing ──────────────────────────────────────────────────────────

fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq as _;
    a.ct_eq(b).into()
}

/// Strip a display-name from an RFC 5322 address, returning only the addr-spec.
///
/// `"Support <support@company.com>"` → `"support@company.com"`
/// `"<support@company.com>"` → `"support@company.com"`
/// `"support@company.com"` → `"support@company.com"` (unchanged)
fn extract_addr_spec(addr: &str) -> String {
    if let Some(start) = addr.find('<')
        && let Some(rel_end) = addr[start + 1..].find('>')
    {
        return addr[start + 1..start + 1 + rel_end].trim().to_string();
    }
    addr.trim().to_string()
}

fn parse_address_list(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(extract_addr_spec)
        .collect()
}

/// Parse and authenticate a Mailgun webhook form POST.
///
/// Returns `Ok(InboundEmail)` on valid signature, `Err(401)` on failure.
pub(crate) fn parse_mailgun(
    form: &HashMap<String, String>,
    signing_key: &str,
) -> Result<InboundEmail, StatusCode> {
    // Refuse all requests when no signing key is configured to prevent trivial forgery.
    if signing_key.is_empty() {
        tracing::error!("inbound_mail.mailgun: signing key is empty; rejecting request");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let timestamp = form.get("timestamp").map_or("", String::as_str);
    let token = form.get("token").map_or("", String::as_str);
    let signature = form.get("signature").map_or("", String::as_str);

    // Reject stale or future timestamps (5-minute window) to block replay attacks.
    let ts: i64 = timestamp.parse().map_err(|_| {
        tracing::warn!("inbound_mail.mailgun: missing or non-numeric timestamp");
        StatusCode::UNAUTHORIZED
    })?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .cast_signed();
    if (now - ts).abs() > 300 {
        tracing::warn!("inbound_mail.mailgun: timestamp outside 5-minute window");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let expected = compute_mailgun_signature(timestamp, token, signing_key);
    if !subtle_eq(expected.as_bytes(), signature.as_bytes()) {
        tracing::warn!("inbound_mail.mailgun: invalid signature — request rejected");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let from = form.get("from").cloned().unwrap_or_default();
    // Collect all recipients: RFC `To` header + Mailgun's envelope `recipient` field.
    let mut to = parse_address_list(form.get("to").map_or("", String::as_str));
    for addr in parse_address_list(form.get("recipient").map_or("", String::as_str)) {
        if !to.contains(&addr) {
            to.push(addr);
        }
    }
    let cc = parse_address_list(
        form.get("Cc")
            .or_else(|| form.get("cc"))
            .map_or("", String::as_str),
    );
    let subject = form.get("subject").cloned().unwrap_or_default();
    let text_body = form.get("body-plain").cloned().filter(|s| !s.is_empty());
    let html_body = form.get("body-html").cloned().filter(|s| !s.is_empty());

    let headers = parse_mailgun_headers(form.get("message-headers").map_or("", String::as_str));

    // Bounce signalling via form field.
    let mut final_headers = headers;
    if let Some(bounced) = form
        .get("X-Mailgun-Bounced-Address")
        .or_else(|| form.get("x-mailgun-bounced-address"))
    {
        final_headers.insert("x-mailgun-bounced-address".to_string(), bounced.clone());
    }

    let spam_score = form
        .get("X-Mailgun-Spam-Score")
        .or_else(|| form.get("x-mailgun-spam-score"))
        .and_then(|s| s.parse::<f64>().ok());
    let spam_verdict = form
        .get("X-Mailgun-Sflag")
        .or_else(|| form.get("x-mailgun-sflag"))
        .cloned();
    let spam_report = if spam_score.is_some() || spam_verdict.is_some() {
        Some(SpamReport {
            score: spam_score,
            verdict: spam_verdict,
        })
    } else {
        None
    };

    // Compact attachment metadata (Mailgun form-field format).
    let attachment_count: usize = form
        .get("attachment-count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
        .min(100);
    let mut attachments = Vec::new();
    for i in 1..=attachment_count {
        if let Some(name) = form.get(&format!("attachment-{i}")) {
            attachments.push(Attachment {
                filename: Some(name.clone()),
                content_type: "application/octet-stream".to_string(),
                data: Bytes::new(),
            });
        }
    }

    Ok(InboundEmail {
        from,
        to,
        cc,
        subject,
        text_body,
        html_body,
        headers: final_headers,
        attachments,
        spam_report,
        raw: Bytes::new(),
        plus_token: None,
    })
}

fn parse_mailgun_headers(json: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if json.is_empty() {
        return map;
    }
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(json) {
        for item in arr {
            if let serde_json::Value::Array(pair) = item
                && pair.len() == 2
                && let (Some(name), Some(value)) = (pair[0].as_str(), pair[1].as_str())
            {
                map.insert(name.to_ascii_lowercase(), value.to_string());
            }
        }
    }
    map
}

/// Parse and authenticate an SES-via-SNS notification.
///
/// Handles `SubscriptionConfirmation` (logs the subscribe URL) and
/// `Notification` (extracts the raw email from the `Message` field).
pub(crate) fn parse_ses(body: &Bytes, headers: &HeaderMap) -> Result<SnsParseResult, StatusCode> {
    let msg_type = headers
        .get("x-amz-sns-message-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match msg_type {
        "SubscriptionConfirmation" => {
            let json: serde_json::Value =
                serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
            let url = json
                .get("SubscribeURL")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            tracing::warn!(
                subscribe_url = %url,
                "inbound_mail.ses: SNS SubscriptionConfirmation received — \
                 SES notifications will NOT be delivered until you visit the \
                 SubscribeURL above to confirm the subscription"
            );
            Ok(SnsParseResult::SubscriptionConfirmation {
                url: url.to_string(),
            })
        }
        "Notification" => {
            let json: serde_json::Value =
                serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
            let message = json.get("Message").and_then(|m| m.as_str()).unwrap_or("");
            // The SNS Message may be (a) a plain base64-encoded RFC 5322 email,
            // (b) a raw RFC 5322 string, or (c) a JSON object with a "content"
            // field containing the base64-encoded email (SES default action format).
            let raw = serde_json::from_str::<serde_json::Value>(message).map_or_else(
                |_| {
                    base64::engine::general_purpose::STANDARD
                        .decode(message)
                        .unwrap_or_else(|_| message.as_bytes().to_vec())
                },
                |msg_json| {
                    msg_json
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map_or_else(
                            || message.as_bytes().to_vec(),
                            |content| {
                                base64::engine::general_purpose::STANDARD
                                    .decode(content)
                                    .unwrap_or_else(|_| content.as_bytes().to_vec())
                            },
                        )
                },
            );
            let email = parse_rfc5322(Bytes::from(raw));
            Ok(SnsParseResult::Email(Box::new(email)))
        }
        _ => {
            tracing::warn!(msg_type, "inbound_mail.ses: unknown SNS message type");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

/// Possible outcomes of parsing an SNS notification body.
#[derive(Debug)]
pub(crate) enum SnsParseResult {
    SubscriptionConfirmation {
        #[allow(dead_code)]
        url: String,
    },
    Email(Box<InboundEmail>),
}

/// Parse and optionally authenticate a generic RFC 5322 raw-body POST.
///
/// When `signing_key` is `Some`, the handler verifies
/// `X-Inbound-Signature: HMAC-SHA256(key, body)` before parsing.
pub(crate) fn parse_generic(
    raw_body: Bytes,
    signing_key: Option<&str>,
    headers: &HeaderMap,
) -> Result<InboundEmail, StatusCode> {
    if let Some(key) = signing_key {
        if key.is_empty() {
            tracing::error!("inbound_mail.generic: signing key is empty; rejecting request");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let provided = headers
            .get("x-inbound-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected = crate::security::config::hmac_sha256_hex(key.as_bytes(), &raw_body);
        if !subtle_eq(expected.as_bytes(), provided.as_bytes()) {
            tracing::warn!("inbound_mail.generic: invalid signature — request rejected");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(parse_rfc5322(raw_body))
}

/// Minimal RFC 5322 parser.
///
/// Handles folded headers, plain-text and HTML bodies (single-part only).
/// Multi-part MIME bodies are accepted but only the first part is extracted.
fn parse_rfc5322(raw: Bytes) -> InboundEmail {
    let text = String::from_utf8_lossy(&raw);

    let (header_block, body_block) = text.find("\r\n\r\n").map_or_else(
        || {
            text.find("\n\n").map_or_else(
                || (text.as_ref(), ""),
                |pos| (&text[..pos], &text[pos + 2..]),
            )
        },
        |pos| (&text[..pos], &text[pos + 4..]),
    );

    let mut from = String::new();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut subject = String::new();
    let mut parsed_headers: HashMap<String, String> = HashMap::new();

    let mut cur_name = String::new();
    let mut cur_value = String::new();

    for line in header_block.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            cur_value.push(' ');
            cur_value.push_str(line.trim());
        } else if let Some(colon) = line.find(':') {
            if !cur_name.is_empty() {
                apply_header(
                    &mut parsed_headers,
                    &mut from,
                    &mut to,
                    &mut cc,
                    &mut subject,
                    &cur_name,
                    &cur_value,
                );
            }
            cur_name = line[..colon].trim().to_ascii_lowercase();
            cur_value = line[colon + 1..].trim().to_string();
        }
    }
    if !cur_name.is_empty() {
        apply_header(
            &mut parsed_headers,
            &mut from,
            &mut to,
            &mut cc,
            &mut subject,
            &cur_name,
            &cur_value,
        );
    }

    let content_type = parsed_headers
        .get("content-type")
        .cloned()
        .unwrap_or_default();
    let cte = parsed_headers
        .get("content-transfer-encoding")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let body_str = body_block.trim().to_string();
    let (text_body, html_body) = if body_str.is_empty() {
        (None, None)
    } else if content_type.starts_with("multipart/") {
        extract_multipart_bodies(&body_str, &content_type)
    } else if content_type.contains("text/html") {
        (None, Some(decode_transfer_encoding(&body_str, &cte)))
    } else {
        (Some(decode_transfer_encoding(&body_str, &cte)), None)
    };

    InboundEmail {
        from,
        to,
        cc,
        subject,
        text_body,
        html_body,
        headers: parsed_headers,
        attachments: Vec::new(),
        spam_report: None,
        raw,
        plus_token: None,
    }
}

/// Decode a MIME part body according to its `Content-Transfer-Encoding` header.
///
/// Handles `base64` and `quoted-printable`; passes everything else through unchanged.
fn decode_transfer_encoding(body: &str, cte: &str) -> String {
    match cte.trim() {
        "base64" => {
            let stripped: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(stripped.as_bytes())
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_else(|| body.to_string())
        }
        "quoted-printable" => decode_quoted_printable(body),
        _ => body.to_string(),
    }
}

/// Decode a quoted-printable encoded string per RFC 2045.
fn decode_quoted_printable(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            if i + 1 < bytes.len() && (bytes[i + 1] == b'\n' || bytes[i + 1] == b'\r') {
                // Soft line break: `=\n` or `=\r\n`
                i += 1;
                if i + 1 < bytes.len() && bytes[i] == b'\r' && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
            } else if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit()
            {
                let hi = (bytes[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
                let lo = (bytes[i + 2] as char).to_digit(16).unwrap_or(0) as u8;
                out.push((hi << 4) | lo);
                i += 3;
            } else {
                out.push(b'=');
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Extract the MIME boundary parameter from a `Content-Type` header value.
fn extract_boundary(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let part = part.trim();
        part.strip_prefix("boundary=")
            .map(|b| b.trim_matches('"').to_string())
    })
}

/// Split a MIME multipart body and return `(text/plain, text/html)` parts.
fn extract_multipart_bodies(body: &str, content_type: &str) -> (Option<String>, Option<String>) {
    let Some(boundary) = extract_boundary(content_type) else {
        return (Some(body.to_string()), None);
    };
    let delimiter = format!("--{boundary}");
    let mut text_body: Option<String> = None;
    let mut html_body: Option<String> = None;

    for part in body.split(&delimiter).skip(1) {
        if part.trim_start_matches('-').trim().is_empty() {
            continue;
        }
        let (part_headers, part_body) = if let Some(pos) = part.find("\r\n\r\n") {
            (&part[..pos], &part[pos + 4..])
        } else if let Some(pos) = part.find("\n\n") {
            (&part[..pos], &part[pos + 2..])
        } else {
            continue;
        };
        let part_ct = part_headers
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
            .map(|l| l[13..].trim().to_ascii_lowercase())
            .unwrap_or_default();
        let part_cte = part_headers
            .lines()
            .find(|l| {
                l.to_ascii_lowercase()
                    .starts_with("content-transfer-encoding:")
            })
            .map(|l| l[26..].trim().to_ascii_lowercase())
            .unwrap_or_default();
        // Strip trailing boundary marker (e.g. "\r\n--boundary--").
        let text = part_body
            .find(&format!("\r\n--{boundary}"))
            .or_else(|| part_body.find(&format!("\n--{boundary}")))
            .map_or(part_body, |end| &part_body[..end])
            .trim();
        if part_ct.starts_with("text/plain") && text_body.is_none() {
            text_body = Some(decode_transfer_encoding(text, &part_cte));
        } else if part_ct.starts_with("text/html") && html_body.is_none() {
            html_body = Some(decode_transfer_encoding(text, &part_cte));
        }
    }
    (text_body, html_body)
}

/// Decode RFC 2047 encoded words (`=?charset?B/Q?text?=`) in a header value.
///
/// Adjacent encoded words separated only by whitespace are joined without the whitespace
/// per RFC 2047 §6.2.
fn decode_rfc2047(value: &str) -> String {
    if !value.contains("=?") {
        return value.to_string();
    }
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    let mut last_was_encoded = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        // If the gap between encoded words is only whitespace, skip it.
        if last_was_encoded && before.chars().all(|c| c == ' ' || c == '\t') {
            // swallow inter-word whitespace
        } else {
            result.push_str(before);
        }
        rest = &rest[start + 2..];
        let Some(charset_end) = rest.find('?') else {
            result.push_str("=?");
            result.push_str(rest);
            return result;
        };
        let charset = &rest[..charset_end];
        rest = &rest[charset_end + 1..];
        let Some(enc_end) = rest.find('?') else {
            result.push_str("=?");
            result.push_str(charset);
            result.push('?');
            result.push_str(rest);
            return result;
        };
        let encoding = &rest[..enc_end];
        rest = &rest[enc_end + 1..];
        let Some(text_end) = rest.find("?=") else {
            result.push_str("=?");
            result.push_str(charset);
            result.push('?');
            result.push_str(encoding);
            result.push('?');
            result.push_str(rest);
            return result;
        };
        let encoded_text = &rest[..text_end];
        rest = &rest[text_end + 2..];
        let decoded_bytes: Option<Vec<u8>> = match encoding.to_ascii_uppercase().as_str() {
            "B" => {
                let stripped: String = encoded_text
                    .chars()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                base64::engine::general_purpose::STANDARD
                    .decode(stripped.as_bytes())
                    .ok()
            }
            "Q" => Some(decode_rfc2047_q(encoded_text.as_bytes())),
            _ => None,
        };
        let decoded = decoded_bytes
            .and_then(|b| rfc2047_bytes_to_string(b, charset))
            .unwrap_or_else(|| encoded_text.to_string());
        result.push_str(&decoded);
        last_was_encoded = true;
    }
    result.push_str(rest);
    result
}

/// Decode RFC 2047 Q-encoding (header variant): `_` → space, `=XX` → byte.
fn decode_rfc2047_q(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'_' {
            out.push(b' ');
            i += 1;
        } else if input[i] == b'='
            && i + 2 < input.len()
            && input[i + 1].is_ascii_hexdigit()
            && input[i + 2].is_ascii_hexdigit()
        {
            let hi = (input[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
            let lo = (input[i + 2] as char).to_digit(16).unwrap_or(0) as u8;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

/// Convert a decoded byte sequence to a String given its RFC 2047 charset label.
fn rfc2047_bytes_to_string(bytes: Vec<u8>, charset: &str) -> Option<String> {
    match charset.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => String::from_utf8(bytes).ok(),
        "iso-8859-1" | "latin-1" | "iso8859-1" => Some(bytes.iter().map(|&b| b as char).collect()),
        _ => String::from_utf8(bytes).ok(),
    }
}

fn apply_header(
    headers: &mut HashMap<String, String>,
    from: &mut String,
    to: &mut Vec<String>,
    cc: &mut Vec<String>,
    subject: &mut String,
    name: &str,
    value: &str,
) {
    headers.insert(name.to_string(), value.to_string());
    match name {
        "from" => *from = value.to_string(),
        "to" => *to = parse_address_list(value),
        "cc" => *cc = parse_address_list(value),
        "subject" => *subject = decode_rfc2047(value),
        _ => {}
    }
}

// ── Axum route construction ───────────────────────────────────────────────────

/// URL-decode a `application/x-www-form-urlencoded` body.
fn url_decode_form(body: &[u8]) -> HashMap<String, String> {
    let s = String::from_utf8_lossy(body);
    serde_urlencoded::from_str(&s).unwrap_or_default()
}

/// Build all Axum routes for the router's configured endpoints.
///
/// Called by `AppBuilder::run()` and `TestApp::build()`.
pub(crate) fn build_routes(
    router: &Arc<InboundMailRouter>,
) -> Vec<(String, axum::Router<crate::state::AppState>)> {
    router
        .endpoints
        .iter()
        .map(|endpoint| {
            let path = endpoint.path.clone();
            let provider = endpoint.provider;
            let signing_key = endpoint.resolve_signing_key();
            let router_arc = Arc::clone(router);

            // Whether a signing key was *configured* (even if the env var is absent).
            let key_configured =
                endpoint.signing_key.is_some() || endpoint.signing_key_env.is_some();
            let axum_router = match provider {
                InboundMailProvider::Mailgun => build_mailgun_route(&path, signing_key, router_arc),
                InboundMailProvider::Ses => build_ses_route(&path, router_arc),
                InboundMailProvider::Generic => {
                    build_generic_route(&path, signing_key, key_configured, router_arc)
                }
            };
            (path, axum_router)
        })
        .collect()
}

fn build_mailgun_route(
    path: &str,
    signing_key: Option<String>,
    router: Arc<InboundMailRouter>,
) -> axum::Router<crate::state::AppState> {
    use axum::extract::DefaultBodyLimit;
    use axum::routing::post;

    let handler = move |_headers: HeaderMap, body: Bytes| {
        let router = Arc::clone(&router);
        let key = signing_key.clone();
        async move {
            let form = url_decode_form(&body);
            let effective_key = key.as_deref().unwrap_or("");
            match parse_mailgun(&form, effective_key) {
                Ok(email) => match router.dispatch(email).await {
                    Ok(()) => StatusCode::OK,
                    Err(e) => {
                        tracing::error!(error = %e, "inbound_mail.mailgun: sync dispatch error");
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                },
                Err(status) => status,
            }
        }
    };

    axum::Router::new()
        .route(path, post(handler))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
}

fn build_ses_route(
    path: &str,
    router: Arc<InboundMailRouter>,
) -> axum::Router<crate::state::AppState> {
    use axum::extract::DefaultBodyLimit;
    use axum::routing::post;

    // One shared client per route for SNS cert fetching.
    #[cfg(feature = "inbound-ses")]
    let http_client = reqwest::Client::new();

    let handler = move |headers: HeaderMap, body: Bytes| {
        let router = Arc::clone(&router);
        #[cfg(feature = "inbound-ses")]
        let http_client = http_client.clone();
        async move {
            // Verify SNS signature before parsing.
            #[cfg(feature = "inbound-ses")]
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                if let Err(status) = sns_verify::verify(&json, &http_client).await {
                    return status;
                }
            }

            match parse_ses(&body, &headers) {
                Ok(SnsParseResult::SubscriptionConfirmation { .. }) => StatusCode::OK,
                Ok(SnsParseResult::Email(email)) => match router.dispatch(*email).await {
                    Ok(()) => StatusCode::OK,
                    Err(e) => {
                        tracing::error!(error = %e, "inbound_mail.ses: sync dispatch error");
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                },
                Err(status) => status,
            }
        }
    };

    axum::Router::new()
        .route(path, post(handler))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
}

fn build_generic_route(
    path: &str,
    signing_key: Option<String>,
    key_configured: bool,
    router: Arc<InboundMailRouter>,
) -> axum::Router<crate::state::AppState> {
    use axum::extract::DefaultBodyLimit;
    use axum::routing::post;

    let handler = move |headers: HeaderMap, body: Bytes| {
        let router = Arc::clone(&router);
        let key = signing_key.clone();
        async move {
            // Fail closed: if a signing key was configured but could not be resolved
            // (e.g. missing env var), reject instead of silently skipping verification.
            if key_configured && key.is_none() {
                tracing::error!(
                    "inbound_mail.generic: signing key env var not resolved; rejecting request"
                );
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            match parse_generic(body, key.as_deref(), &headers) {
                Ok(email) => match router.dispatch(email).await {
                    Ok(()) => StatusCode::OK,
                    Err(e) => {
                        tracing::error!(error = %e, "inbound_mail.generic: sync dispatch error");
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                },
                Err(status) => status,
            }
        }
    };

    axum::Router::new()
        .route(path, post(handler))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailgun_signature_is_64_hex_chars() {
        let sig = compute_mailgun_signature("1234", "token", "key");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn mailgun_signature_is_deterministic() {
        assert_eq!(
            compute_mailgun_signature("ts", "tok", "key"),
            compute_mailgun_signature("ts", "tok", "key"),
        );
    }

    #[test]
    fn mailgun_signature_changes_with_input() {
        let a = compute_mailgun_signature("ts1", "tok", "key");
        let b = compute_mailgun_signature("ts2", "tok", "key");
        assert_ne!(a, b);
    }

    #[test]
    fn exact_pattern_case_insensitive() {
        let p = RecipientPattern::Exact("Support@Company.COM".to_string());
        assert!(p.matches("support@company.com"));
        assert!(p.matches("SUPPORT@COMPANY.COM"));
        assert!(!p.matches("other@company.com"));
    }

    #[test]
    fn local_prefix_matches_with_and_without_plus() {
        let p = RecipientPattern::LocalPrefix("ticket".to_string());
        assert!(p.matches("ticket@example.com"));
        assert!(p.matches("ticket+123@example.com"));
        assert!(!p.matches("my-ticket@example.com"));
    }

    #[test]
    fn plus_address_extracts_token() {
        let p = RecipientPattern::PlusAddress {
            local: "replies".to_string(),
            domain: Some("app.example".to_string()),
        };
        assert!(p.matches("replies+abc@app.example"));
        assert!(!p.matches("other+abc@app.example"));
        assert!(!p.matches("replies@app.example"));
        assert_eq!(
            p.extract_token("replies+xyz@app.example"),
            Some("xyz".to_string())
        );
    }

    #[test]
    fn plus_address_without_domain_restriction() {
        let p = RecipientPattern::PlusAddress {
            local: "mail".to_string(),
            domain: None,
        };
        assert!(p.matches("mail+token@any.org"));
        assert_eq!(
            p.extract_token("mail+mytoken@anything.com"),
            Some("mytoken".to_string())
        );
    }

    #[test]
    fn any_pattern_matches_everything() {
        let p = RecipientPattern::Any;
        assert!(p.matches("anyone@example.com"));
        assert!(p.matches(""));
        assert!(p.extract_token("irrelevant@example.com").is_none());
    }

    #[test]
    fn rfc5322_parses_basic_email() {
        let raw = "From: alice@example.com\r\n\
                   To: bob@example.com\r\n\
                   Subject: Hello\r\n\
                   \r\n\
                   Body text here.";
        let email = parse_rfc5322(Bytes::from_static(raw.as_bytes()));
        assert_eq!(email.from, "alice@example.com");
        assert_eq!(email.to, vec!["bob@example.com"]);
        assert_eq!(email.subject, "Hello");
        assert_eq!(email.text_body.as_deref(), Some("Body text here."));
    }

    #[test]
    fn rfc5322_parses_multiple_to_addresses() {
        let raw = "From: alice@example.com\r\n\
                   To: bob@example.com, carol@example.com\r\n\
                   Subject: Multi\r\n\
                   \r\n";
        let email = parse_rfc5322(Bytes::from_static(raw.as_bytes()));
        assert_eq!(email.to.len(), 2);
        assert!(email.to.contains(&"bob@example.com".to_string()));
        assert!(email.to.contains(&"carol@example.com".to_string()));
    }

    #[test]
    fn generic_config_no_signing_key() {
        let c = InboundMailEndpointConfig::generic("/inbound");
        assert!(c.signing_key.is_none());
        assert_eq!(c.provider, InboundMailProvider::Generic);
    }

    #[test]
    fn config_resolve_signing_key_prefers_literal() {
        let c = InboundMailEndpointConfig {
            signing_key: Some("literal".to_string()),
            signing_key_env: Some("SOME_VAR".to_string()),
            ..InboundMailEndpointConfig::generic("/p")
        };
        assert_eq!(c.resolve_signing_key().as_deref(), Some("literal"));
    }

    #[test]
    fn config_resolve_signing_key_falls_back_to_env() {
        temp_env::with_var("TEST_AUTUMN_INBOUND_KEY", Some("from-env"), || {
            let c = InboundMailEndpointConfig {
                signing_key: None,
                signing_key_env: Some("TEST_AUTUMN_INBOUND_KEY".to_string()),
                ..InboundMailEndpointConfig::generic("/p")
            };
            assert_eq!(c.resolve_signing_key().as_deref(), Some("from-env"));
        });
    }

    fn now_ts() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
            .to_string()
    }

    #[test]
    fn mailgun_parse_rejects_invalid_signature() {
        let ts = now_ts();
        let form: HashMap<String, String> = [
            ("from".to_string(), "user@example.com".to_string()),
            ("to".to_string(), "support@company.com".to_string()),
            ("subject".to_string(), "Test".to_string()),
            ("timestamp".to_string(), ts),
            ("token".to_string(), "some-token".to_string()),
            ("signature".to_string(), "deadbeef".repeat(8)),
        ]
        .into_iter()
        .collect();

        let result = parse_mailgun(&form, "correct-key");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn mailgun_parse_accepts_valid_signature() {
        let ts = now_ts();
        let tok = "mytoken";
        let key = "my-signing-key";
        let sig = compute_mailgun_signature(&ts, tok, key);

        let form: HashMap<String, String> = [
            ("from".to_string(), "user@example.com".to_string()),
            ("to".to_string(), "support@company.com".to_string()),
            ("subject".to_string(), "Hello".to_string()),
            ("body-plain".to_string(), "Test body".to_string()),
            ("timestamp".to_string(), ts),
            ("token".to_string(), tok.to_string()),
            ("signature".to_string(), sig),
        ]
        .into_iter()
        .collect();

        let result = parse_mailgun(&form, key);
        assert!(result.is_ok());
        let email = result.unwrap();
        assert_eq!(email.from, "user@example.com");
        assert_eq!(email.to, vec!["support@company.com"]);
        assert_eq!(email.subject, "Hello");
        assert_eq!(email.text_body.as_deref(), Some("Test body"));
    }

    #[test]
    fn mailgun_parse_detects_bounce_header() {
        let ts = now_ts();
        let tok = "bouncetoken";
        let key = "bounce-key";
        let sig = compute_mailgun_signature(&ts, tok, key);

        let form: HashMap<String, String> = [
            ("from".to_string(), "MAILER-DAEMON@mailgun.net".to_string()),
            ("to".to_string(), "bounced@company.com".to_string()),
            ("subject".to_string(), "Delivery failed".to_string()),
            ("timestamp".to_string(), ts),
            ("token".to_string(), tok.to_string()),
            ("signature".to_string(), sig),
            (
                "X-Mailgun-Bounced-Address".to_string(),
                "user@bad-domain.com".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let email = parse_mailgun(&form, key).unwrap();
        assert!(email.headers.contains_key("x-mailgun-bounced-address"));
    }

    #[test]
    fn mailgun_empty_key_returns_500() {
        let result = parse_mailgun(&HashMap::new(), "");
        assert_eq!(result.unwrap_err(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn mailgun_merges_recipient_into_to() {
        let ts = now_ts();
        let tok = "tok";
        let key = "k";
        let sig = compute_mailgun_signature(&ts, tok, key);
        let form: HashMap<String, String> = [
            ("from".to_string(), "u@example.com".to_string()),
            ("to".to_string(), "a@example.com".to_string()),
            ("recipient".to_string(), "b@example.com".to_string()),
            ("timestamp".to_string(), ts),
            ("token".to_string(), tok.to_string()),
            ("signature".to_string(), sig),
        ]
        .into_iter()
        .collect();
        let email = parse_mailgun(&form, key).unwrap();
        assert!(email.to.contains(&"a@example.com".to_string()));
        assert!(email.to.contains(&"b@example.com".to_string()));
    }

    #[test]
    fn mailgun_display_name_stripped_from_to() {
        let ts = now_ts();
        let tok = "tok2";
        let key = "k2";
        let sig = compute_mailgun_signature(&ts, tok, key);
        let form: HashMap<String, String> = [
            ("from".to_string(), "u@example.com".to_string()),
            (
                "to".to_string(),
                "Support <support@company.com>".to_string(),
            ),
            ("timestamp".to_string(), ts),
            ("token".to_string(), tok.to_string()),
            ("signature".to_string(), sig),
        ]
        .into_iter()
        .collect();
        let email = parse_mailgun(&form, key).unwrap();
        assert!(email.to.contains(&"support@company.com".to_string()));
    }

    #[test]
    fn mailgun_spam_report_populated() {
        let ts = now_ts();
        let tok = "spam-tok";
        let key = "spam-key";
        let sig = compute_mailgun_signature(&ts, tok, key);
        let form: HashMap<String, String> = [
            ("from".to_string(), "u@example.com".to_string()),
            ("to".to_string(), "r@example.com".to_string()),
            ("timestamp".to_string(), ts),
            ("token".to_string(), tok.to_string()),
            ("signature".to_string(), sig),
            ("X-Mailgun-Spam-Score".to_string(), "5.7".to_string()),
            ("X-Mailgun-Sflag".to_string(), "Yes".to_string()),
        ]
        .into_iter()
        .collect();
        let email = parse_mailgun(&form, key).unwrap();
        let report = email.spam_report.unwrap();
        assert!((report.score.unwrap() - 5.7_f64).abs() < 0.01);
        assert_eq!(report.verdict.as_deref(), Some("Yes"));
    }

    #[test]
    fn generic_empty_key_returns_500() {
        let headers = HeaderMap::new();
        let result = parse_generic(Bytes::from("test"), Some(""), &headers);
        assert_eq!(result.unwrap_err(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn generic_invalid_signature_returns_401() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::HeaderName::from_static("x-inbound-signature"),
            "badhex".parse().unwrap(),
        );
        let result = parse_generic(Bytes::from("body"), Some("key"), &headers);
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn generic_no_key_passes_through() {
        let headers = HeaderMap::new();
        let raw = "From: a@b.com\r\nTo: c@d.com\r\nSubject: S\r\n\r\nB";
        let result = parse_generic(Bytes::from(raw), None, &headers);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().subject, "S");
    }

    #[test]
    fn parse_ses_subscription_confirmation() {
        let headers = HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x-amz-sns-message-type"),
            "SubscriptionConfirmation".parse().unwrap(),
        )]);
        let payload = serde_json::json!({
            "Type": "SubscriptionConfirmation",
            "SubscribeURL": "https://sns.example.com/confirm?token=abc"
        });
        let body = Bytes::from(payload.to_string());
        let result = parse_ses(&body, &headers);
        assert!(
            matches!(result, Ok(SnsParseResult::SubscriptionConfirmation { .. })),
            "expected SubscriptionConfirmation"
        );
    }

    #[test]
    fn parse_ses_notification_base64_email() {
        use base64::Engine as _;
        let raw =
            "From: sender@example.com\r\nTo: recv@example.com\r\nSubject: SES-Base64\r\n\r\nHi";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let headers = HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x-amz-sns-message-type"),
            "Notification".parse().unwrap(),
        )]);
        let sns = serde_json::json!({ "Type": "Notification", "Message": encoded });
        let result = parse_ses(&Bytes::from(sns.to_string()), &headers);
        let Ok(SnsParseResult::Email(email)) = result else {
            panic!("expected Email result, got: {result:?}");
        };
        assert_eq!(email.subject, "SES-Base64");
    }

    #[test]
    fn parse_ses_notification_nested_content_field() {
        use base64::Engine as _;
        let raw = "From: sender@example.com\r\nTo: dest@example.com\r\nSubject: Nested\r\n\r\nBody";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let msg_json = serde_json::json!({ "content": encoded });
        let headers = HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x-amz-sns-message-type"),
            "Notification".parse().unwrap(),
        )]);
        let sns = serde_json::json!({
            "Type": "Notification",
            "Message": msg_json.to_string()
        });
        let result = parse_ses(&Bytes::from(sns.to_string()), &headers);
        let Ok(SnsParseResult::Email(email)) = result else {
            panic!("expected Email, got: {result:?}");
        };
        assert_eq!(email.subject, "Nested");
    }

    #[test]
    fn parse_ses_unknown_type_returns_400() {
        let headers = HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x-amz-sns-message-type"),
            "UnknownType".parse().unwrap(),
        )]);
        let result = parse_ses(&Bytes::from("{}"), &headers);
        assert_eq!(result.unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn decode_rfc2047_plain_subject_unchanged() {
        assert_eq!(decode_rfc2047("Hello world"), "Hello world");
    }

    #[test]
    fn decode_rfc2047_base64_utf8() {
        use base64::Engine as _;
        let text = "Héllo";
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let encoded = format!("=?UTF-8?B?{b64}?=");
        assert_eq!(decode_rfc2047(&encoded), text);
    }

    #[test]
    fn decode_rfc2047_q_encoding() {
        // =?UTF-8?Q?Hello_World?= → "Hello World"
        let encoded = "=?UTF-8?Q?Hello_World?=";
        assert_eq!(decode_rfc2047(encoded), "Hello World");
    }

    #[test]
    fn decode_rfc2047_q_hex_sequence() {
        // =?UTF-8?Q?caf=C3=A9?= → "café"
        let encoded = "=?UTF-8?Q?caf=C3=A9?=";
        assert_eq!(decode_rfc2047(encoded), "café");
    }

    #[test]
    fn decode_rfc2047_adjacent_words_no_space() {
        // Two adjacent encoded words should be joined without whitespace.
        use base64::Engine as _;
        let w1 = base64::engine::general_purpose::STANDARD.encode("Hello");
        let w2 = base64::engine::general_purpose::STANDARD.encode(" World");
        let encoded = format!("=?UTF-8?B?{w1}?= =?UTF-8?B?{w2}?=");
        assert_eq!(decode_rfc2047(&encoded), "Hello World");
    }

    #[test]
    fn decode_rfc2047_mixed_literal_and_encoded() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode("World");
        let encoded = format!("Hello =?UTF-8?B?{b64}?=");
        assert_eq!(decode_rfc2047(&encoded), "Hello World");
    }

    #[test]
    fn decode_transfer_encoding_passthrough_for_7bit() {
        assert_eq!(
            decode_transfer_encoding("Hello world", "7bit"),
            "Hello world"
        );
    }

    #[test]
    fn decode_transfer_encoding_base64_decodes() {
        use base64::Engine as _;
        let plain = "Hello, MIME world!";
        let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
        assert_eq!(decode_transfer_encoding(&encoded, "base64"), plain);
    }

    #[test]
    fn decode_transfer_encoding_base64_with_whitespace() {
        // Base64 encoded across lines (as email clients produce).
        use base64::Engine as _;
        let plain = "Hello";
        let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
        let with_ws = format!("{encoded}\r\n");
        assert_eq!(decode_transfer_encoding(&with_ws, "base64"), plain);
    }

    #[test]
    fn decode_transfer_encoding_quoted_printable() {
        let qp = "Subject=3A Hello=20World";
        assert_eq!(
            decode_transfer_encoding(qp, "quoted-printable"),
            "Subject: Hello World"
        );
    }

    #[test]
    fn decode_quoted_printable_soft_line_break() {
        let qp = "long line=\r\ncontinued";
        assert_eq!(decode_quoted_printable(qp), "long linecontinued");
    }

    #[test]
    fn decode_quoted_printable_literal_equals() {
        let qp = "price=3D10";
        assert_eq!(decode_quoted_printable(qp), "price=10");
    }

    #[test]
    fn extract_multipart_bodies_base64_decoded() {
        use base64::Engine as _;
        let b = "boundary99";
        let plain = "Decoded body text";
        let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
        let body = format!(
            "--{b}\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n--{b}--\r\n"
        );
        let ct = format!("multipart/mixed; boundary={b}");
        let (text, _html) = extract_multipart_bodies(&body, &ct);
        assert_eq!(text.unwrap(), plain);
    }

    #[test]
    fn extract_multipart_bodies_quoted_printable_decoded() {
        let b = "bnd";
        let body = format!(
            "--{b}\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHello=20World\r\n--{b}--\r\n"
        );
        let ct = format!("multipart/mixed; boundary={b}");
        let (text, _html) = extract_multipart_bodies(&body, &ct);
        assert_eq!(text.unwrap(), "Hello World");
    }

    #[test]
    fn extract_boundary_parses_unquoted() {
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=abc123"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_boundary_parses_quoted() {
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=\"abc=123\""),
            Some("abc=123".to_string())
        );
    }

    #[test]
    fn extract_boundary_returns_none_for_plain_content_type() {
        assert!(extract_boundary("text/plain").is_none());
    }

    #[test]
    fn extract_multipart_bodies_text_and_html() {
        let b = "boundary42";
        let body = format!(
            "--{b}\r\nContent-Type: text/plain\r\n\r\nHello text\r\n\
             --{b}\r\nContent-Type: text/html\r\n\r\n<b>Hello</b>\r\n\
             --{b}--\r\n"
        );
        let ct = format!("multipart/alternative; boundary={b}");
        let (text, html) = extract_multipart_bodies(&body, &ct);
        assert_eq!(text.as_deref(), Some("Hello text"));
        assert_eq!(html.as_deref(), Some("<b>Hello</b>"));
    }

    #[test]
    fn extract_multipart_bodies_no_boundary_returns_body_as_text() {
        let (text, html) = extract_multipart_bodies("plain text", "text/plain");
        assert_eq!(text.as_deref(), Some("plain text"));
        assert!(html.is_none());
    }

    #[test]
    fn rfc5322_parses_multipart_body() {
        let b = "TESTBOUNDARY";
        let raw = format!(
            "From: a@b.com\r\nTo: c@d.com\r\nSubject: Multi\r\n\
             Content-Type: multipart/alternative; boundary={b}\r\n\r\n\
             --{b}\r\nContent-Type: text/plain\r\n\r\nText part\r\n\
             --{b}\r\nContent-Type: text/html\r\n\r\n<p>HTML</p>\r\n\
             --{b}--\r\n"
        );
        let email = parse_rfc5322(Bytes::from(raw));
        assert_eq!(email.text_body.as_deref(), Some("Text part"));
        assert_eq!(email.html_body.as_deref(), Some("<p>HTML</p>"));
    }

    #[test]
    fn rfc5322_html_only_body() {
        let raw = "From: a@b.com\r\nTo: c@d.com\r\nContent-Type: text/html\r\n\r\n<p>Hello</p>";
        let email = parse_rfc5322(Bytes::from_static(raw.as_bytes()));
        assert!(email.text_body.is_none());
        assert_eq!(email.html_body.as_deref(), Some("<p>Hello</p>"));
    }

    #[test]
    fn primary_recipient_returns_first_to() {
        let email = InboundEmail {
            from: "a@b.com".to_string(),
            to: vec!["first@x.com".to_string(), "second@x.com".to_string()],
            cc: vec![],
            subject: String::new(),
            text_body: None,
            html_body: None,
            headers: HashMap::new(),
            attachments: vec![],
            spam_report: None,
            raw: Bytes::new(),
            plus_token: None,
        };
        assert_eq!(email.primary_recipient(), Some("first@x.com"));
    }

    #[test]
    fn primary_recipient_returns_none_for_empty_to() {
        let email = InboundEmail {
            from: "a@b.com".to_string(),
            to: vec![],
            cc: vec![],
            subject: String::new(),
            text_body: None,
            html_body: None,
            headers: HashMap::new(),
            attachments: vec![],
            spam_report: None,
            raw: Bytes::new(),
            plus_token: None,
        };
        assert!(email.primary_recipient().is_none());
    }
}
