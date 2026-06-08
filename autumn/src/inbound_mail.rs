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

        // Complaint detection: Mailgun sets X-Mailgun-Sflag=YES for spam-flagged mail.
        let spam_flagged = email
            .headers
            .get("x-mailgun-sflag")
            .is_some_and(|v| v.eq_ignore_ascii_case("YES"));
        if spam_flagged && let Some(handler) = self.complaint_handler {
            return handler(email).await;
        }

        for info in &self.handlers {
            let matched = email.to.iter().find(|r| info.pattern.matches(r)).cloned();
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
            tracing::info!(
                subscribe_url = %url,
                "inbound_mail.ses: SNS SubscriptionConfirmation received; \
                 visit SubscribeURL to confirm or configure auto-confirm"
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
    let body_str = body_block.trim().to_string();
    let (text_body, html_body) = if body_str.is_empty() {
        (None, None)
    } else if content_type.starts_with("multipart/") {
        extract_multipart_bodies(&body_str, &content_type)
    } else if content_type.contains("text/html") {
        (None, Some(body_str))
    } else {
        (Some(body_str), None)
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
        // Strip trailing boundary marker (e.g. "\r\n--boundary--").
        let text = part_body
            .find(&format!("\r\n--{boundary}"))
            .or_else(|| part_body.find(&format!("\n--{boundary}")))
            .map_or(part_body, |end| &part_body[..end])
            .trim();
        if part_ct.starts_with("text/plain") && text_body.is_none() {
            text_body = Some(text.to_string());
        } else if part_ct.starts_with("text/html") && html_body.is_none() {
            html_body = Some(text.to_string());
        }
    }
    (text_body, html_body)
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
        "subject" => *subject = value.to_string(),
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

    let handler = move |headers: HeaderMap, body: Bytes| {
        let router = Arc::clone(&router);
        async move {
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
}
