//! Transactional email support.
//!
//! The public surface is intentionally small: build a [`Mail`] value, send it
//! through the cloneable [`Mailer`] extractor, and swap transports through the
//! [`MailTransport`] trait when SMTP is not the right coffin lining.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::response::{Html, IntoResponse, Response};
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use thiserror::Error;

use crate::{AppState, AutumnError, AutumnResult};

/// Mail transport selected by `[mail].transport`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Write full email contents to the tracing log at INFO.
    Log,
    /// Write RFC 822 `.eml` files under `target/mail` or a configured dir.
    File,
    /// Send through SMTP using Lettre.
    Smtp,
    /// Drop all email sends successfully.
    #[default]
    Disabled,
}

impl Transport {
    pub(crate) fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "log" => Some(Self::Log),
            "file" => Some(Self::File),
            "smtp" => Some(Self::Smtp),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// SMTP TLS mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Plain connection; useful only for local test SMTP sinks.
    Disabled,
    /// Upgrade with STARTTLS.
    #[default]
    StartTls,
    /// Connect with wrapper TLS.
    Tls,
}

impl TlsMode {
    pub(crate) fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "disabled" => Some(Self::Disabled),
            "starttls" | "start_tls" => Some(Self::StartTls),
            "tls" => Some(Self::Tls),
            _ => None,
        }
    }
}

/// SMTP configuration nested under `[mail.smtp]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SmtpConfig {
    /// SMTP host name.
    #[serde(default)]
    pub host: Option<String>,
    /// SMTP port. Defaults to 587 for STARTTLS, 465 for TLS, and 25 for disabled TLS.
    #[serde(default)]
    pub port: Option<u16>,
    /// Optional SMTP username.
    #[serde(default)]
    pub username: Option<String>,
    /// Environment variable containing the SMTP password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// TLS behavior.
    #[serde(default)]
    pub tls: TlsMode,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: None,
            port: None,
            username: None,
            password_env: None,
            tls: TlsMode::StartTls,
        }
    }
}

/// `[mail]` config section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MailConfig {
    /// Active transport.
    #[serde(default)]
    pub transport: Transport,
    /// Default From header.
    #[serde(default)]
    pub from: Option<String>,
    /// Default Reply-To header.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Permit log transport in `prod`.
    #[serde(default)]
    pub allow_log_in_production: bool,
    /// Acknowledge that `deliver_later` may use the in-process Tokio fallback in
    /// `prod`. Without a registered durable [`MailDeliveryQueue`], this is the
    /// only way to start the app in `prod` with an active mail transport.
    #[serde(default)]
    pub allow_in_process_deliver_later_in_production: bool,
    /// Directory for file transport.
    #[serde(default = "default_file_dir")]
    pub file_dir: PathBuf,
    /// Force-enable the dev mail preview UI.
    ///
    /// The UI is auto-enabled in `dev` when `mail.transport = "file"`.
    /// Setting this flag outside `dev` is rejected at startup.
    #[serde(default)]
    pub preview: bool,
    /// SMTP settings.
    #[serde(default)]
    pub smtp: SmtpConfig,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            transport: Transport::Disabled,
            from: None,
            reply_to: None,
            allow_log_in_production: false,
            allow_in_process_deliver_later_in_production: false,
            file_dir: default_file_dir(),
            preview: false,
            smtp: SmtpConfig::default(),
        }
    }
}

impl MailConfig {
    /// Validate semantic mail configuration.
    ///
    /// # Errors
    ///
    /// Returns [`crate::config::ConfigError::Validation`] for unsafe profile
    /// combinations or missing SMTP settings.
    pub fn validate(&self, profile: Option<&str>) -> Result<(), crate::config::ConfigError> {
        if matches!(profile, Some("prod" | "production"))
            && self.transport == Transport::Log
            && !self.allow_log_in_production
        {
            return Err(crate::config::ConfigError::Validation(
                "mail.transport = \"log\" is disabled in prod; set mail.allow_log_in_production = true to acknowledge this explicitly".to_owned(),
            ));
        }

        if self.transport == Transport::Smtp
            && self.smtp.host.as_deref().map_or("", str::trim).is_empty()
        {
            return Err(crate::config::ConfigError::Validation(
                "mail.smtp.host is required when mail.transport = \"smtp\"".to_owned(),
            ));
        }

        if self.preview && !matches!(profile, Some("dev" | "development")) {
            return Err(crate::config::ConfigError::Validation(
                "mail.preview = true is only allowed in dev; refusing to mount /_autumn/mail outside the dev profile".to_owned(),
            ));
        }

        Ok(())
    }

    pub(crate) fn preview_routes_enabled(&self, profile: Option<&str>) -> bool {
        matches!(profile, Some("dev" | "development"))
            && (self.preview || self.transport == Transport::File)
    }
}

fn default_file_dir() -> PathBuf {
    PathBuf::from("target/mail")
}

/// Renderable mail body input.
pub trait IntoMailBody {
    /// Convert into owned body text.
    fn into_mail_body(self) -> String;
}

impl IntoMailBody for String {
    fn into_mail_body(self) -> String {
        self
    }
}

impl IntoMailBody for &str {
    fn into_mail_body(self) -> String {
        self.to_owned()
    }
}

impl IntoMailBody for maud::Markup {
    fn into_mail_body(self) -> String {
        self.into_string()
    }
}

/// A transactional email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    /// Optional From header. Falls back to [`Mailer`]'s default.
    pub from: Option<String>,
    /// Optional Reply-To header. Falls back to [`Mailer`]'s default.
    pub reply_to: Option<String>,
    /// To recipients.
    pub to: Vec<String>,
    /// Subject header.
    pub subject: String,
    /// HTML body.
    pub html: Option<String>,
    /// Plain-text body.
    pub text: Option<String>,
}

/// Stable root path for the dev mail preview UI.
pub const MAIL_PREVIEW_PATH: &str = "/_autumn/mail";

const MAIL_PREVIEW_MESSAGE_PATH: &str = "/_autumn/mail/messages/{message_id}";
const MAIL_PREVIEW_TEMPLATE_PATH: &str = "/_autumn/mail/previews/{mailer}/{method}";

/// A developer-authored, zero-argument mail template preview.
#[derive(Clone)]
pub struct MailPreview {
    mailer: &'static str,
    method: &'static str,
    render: fn() -> Mail,
}

impl std::fmt::Debug for MailPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailPreview")
            .field("mailer", &self.mailer)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl MailPreview {
    /// Register a mail preview for the dev mail preview UI.
    #[must_use]
    pub const fn new(mailer: &'static str, method: &'static str, render: fn() -> Mail) -> Self {
        Self {
            mailer,
            method,
            render,
        }
    }

    /// Mailer type label used in preview URLs.
    #[must_use]
    pub const fn mailer(&self) -> &'static str {
        self.mailer
    }

    /// Preview method label used in preview URLs.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Render the preview without invoking any configured transport.
    ///
    /// # Errors
    ///
    /// Returns [`MailPreviewError::PreviewPanicked`] if the preview function
    /// panics while constructing sample data.
    pub fn render(&self) -> Result<Mail, MailPreviewError> {
        std::panic::catch_unwind(|| (self.render)()).map_err(|_| {
            MailPreviewError::PreviewPanicked {
                mailer: self.mailer,
                method: self.method,
            }
        })
    }
}

/// Collection of registered mail previews stored on [`AppState`].
#[derive(Debug, Clone, Default)]
pub struct MailPreviewRegistry {
    previews: Arc<Vec<MailPreview>>,
}

impl MailPreviewRegistry {
    /// Create a registry from preview registrations.
    #[must_use]
    pub fn new(previews: Vec<MailPreview>) -> Self {
        Self {
            previews: Arc::new(previews),
        }
    }

    /// Registered previews.
    #[must_use]
    pub fn previews(&self) -> &[MailPreview] {
        &self.previews
    }

    fn find(&self, mailer: &str, method: &str) -> Option<MailPreview> {
        self.previews
            .iter()
            .find(|preview| preview.mailer == mailer && preview.method == method)
            .cloned()
    }
}

/// Dev mail preview UI errors.
#[derive(Debug, Error)]
pub enum MailPreviewError {
    /// File transport preview IO failed.
    #[error("mail preview file IO failed: {0}")]
    Io(#[from] std::io::Error),
    /// Requested captured message was not found.
    #[error("captured mail message not found: {0}")]
    NotFound(String),
    /// Requested message id is not a single `.eml` filename.
    #[error("invalid captured mail message id: {0}")]
    InvalidMessageId(String),
    /// Developer-authored preview panicked while rendering sample data.
    #[error("mail preview {mailer}::{method} panicked while rendering")]
    PreviewPanicked {
        /// Mailer label.
        mailer: &'static str,
        /// Method label.
        method: &'static str,
    },
}

impl Mail {
    /// Start building a mail message.
    #[must_use]
    pub fn builder() -> MailBuilder {
        MailBuilder::default()
    }

    fn with_defaults(mut self, defaults: &MailerDefaults) -> Self {
        if self.from.is_none() {
            self.from.clone_from(&defaults.from);
        }
        if self.reply_to.is_none() {
            self.reply_to.clone_from(&defaults.reply_to);
        }
        self
    }
}

/// Builder for [`Mail`].
#[derive(Debug, Clone, Default)]
pub struct MailBuilder {
    from: Option<String>,
    reply_to: Option<String>,
    to: Vec<String>,
    subject: Option<String>,
    html: Option<String>,
    text: Option<String>,
}

impl MailBuilder {
    /// Set a message-specific From header.
    #[must_use]
    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    /// Set a message-specific Reply-To header.
    #[must_use]
    pub fn reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// Add a To recipient.
    #[must_use]
    pub fn to(mut self, to: impl Into<String>) -> Self {
        self.to.push(to.into());
        self
    }

    /// Set the subject.
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the HTML body.
    #[must_use]
    pub fn html(mut self, html: impl IntoMailBody) -> Self {
        self.html = Some(html.into_mail_body());
        self
    }

    /// Set the plain-text body.
    #[must_use]
    pub fn text(mut self, text: impl IntoMailBody) -> Self {
        self.text = Some(text.into_mail_body());
        self
    }

    /// Build the mail.
    ///
    /// # Errors
    ///
    /// Returns [`MailError::InvalidMessage`] when required fields are missing.
    pub fn build(self) -> Result<Mail, MailError> {
        if self.to.is_empty() {
            return Err(MailError::InvalidMessage(
                "mail must have at least one recipient".to_owned(),
            ));
        }
        let subject = self
            .subject
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| MailError::InvalidMessage("mail subject is required".to_owned()))?;
        if self.html.is_none() && self.text.is_none() {
            return Err(MailError::InvalidMessage(
                "mail must include html or text body".to_owned(),
            ));
        }
        Ok(Mail {
            from: self.from,
            reply_to: self.reply_to,
            to: self.to,
            subject,
            html: self.html,
            text: self.text,
        })
    }
}

/// Mailer errors.
#[derive(Debug, Error)]
pub enum MailError {
    /// Message could not be built or validated.
    #[error("invalid mail message: {0}")]
    InvalidMessage(String),
    /// Deferred delivery could not be scheduled.
    #[error("mail runtime unavailable: {0}")]
    RuntimeUnavailable(String),
    /// Address parsing failed.
    #[error("invalid mail address {address:?}: {source}")]
    InvalidAddress {
        /// Address that failed to parse.
        address: String,
        /// Lettre parse error.
        source: lettre::address::AddressError,
    },
    /// Lettre message construction failed.
    #[error("failed to build mail message: {0}")]
    Build(#[from] lettre::error::Error),
    /// SMTP transport failed.
    #[error("smtp send failed: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),
    /// File transport failed.
    #[error("file mail transport failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Escape hatch for custom transports.
pub trait MailTransport: Send + Sync {
    /// Send a mail message.
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

    /// Returns `true` if this transport is intentionally a no-op (e.g.
    /// [`Transport::Disabled`] for review apps and tests).
    ///
    /// When `true`, [`Mailer::deliver_later`] short-circuits before the queue
    /// or in-process fallback so deferred mail honors the same "drop
    /// everything" contract as immediate sends. Custom transports that mean
    /// "drop all mail" can override this to opt into the same behavior; the
    /// default of `false` preserves the existing contract for transports that
    /// merely capture mail (file, log, etc.) or send it (SMTP, custom APIs).
    fn is_disabled(&self) -> bool {
        false
    }
}

/// Durable backend for [`Mailer::deliver_later`].
///
/// Implementors persist the mail (DB row, Redis stream, Harvest job, etc.) and
/// return as soon as the handoff is durable. The framework's in-process Tokio
/// fallback is intentionally not durable; production deployments should
/// register a real implementation via [`MailDeliveryQueueHandle`] before
/// `install_mailer` runs, or set
/// [`MailConfig::allow_in_process_deliver_later_in_production`] to opt into the
/// fallback explicitly.
pub trait MailDeliveryQueue: Send + Sync {
    /// Enqueue a mail for durable later delivery.
    fn enqueue<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;
}

/// Cloneable handle to a [`MailDeliveryQueue`].
///
/// Designed for storage on [`AppState`] extensions. Plugins
/// (Harvest, custom Redis, etc.) install this before `install_mailer` runs and
/// the mailer picks it up.
#[derive(Clone)]
pub struct MailDeliveryQueueHandle(Arc<dyn MailDeliveryQueue>);

impl MailDeliveryQueueHandle {
    /// Wrap a queue implementation in a cloneable handle.
    #[must_use]
    pub fn new(queue: impl MailDeliveryQueue + 'static) -> Self {
        Self(Arc::new(queue))
    }

    /// Wrap an already-shared queue implementation.
    #[must_use]
    pub fn from_arc(queue: Arc<dyn MailDeliveryQueue>) -> Self {
        Self(queue)
    }

    /// Borrow the inner queue.
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn MailDeliveryQueue> {
        &self.0
    }
}

impl std::fmt::Debug for MailDeliveryQueueHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailDeliveryQueueHandle").finish()
    }
}

#[derive(Debug, Clone, Default)]
struct MailerDefaults {
    from: Option<String>,
    reply_to: Option<String>,
}

/// Cloneable email sender. Extract it in handlers as `mailer: Mailer`.
#[derive(Clone)]
pub struct Mailer {
    defaults: Arc<MailerDefaults>,
    transport: Arc<dyn MailTransport>,
    delivery_queue: Option<Arc<dyn MailDeliveryQueue>>,
}

impl Mailer {
    /// Build a mailer manually.
    #[must_use]
    pub fn builder() -> MailerBuilder {
        MailerBuilder::default()
    }

    /// Build a mailer from resolved config.
    ///
    /// # Errors
    ///
    /// Returns an error when SMTP or address configuration is invalid.
    pub fn from_config(config: &MailConfig) -> Result<Self, MailError> {
        let mut builder = Self::builder().transport(config.transport);
        if let Some(from) = &config.from {
            builder = builder.from(from.clone());
        }
        if let Some(reply_to) = &config.reply_to {
            builder = builder.reply_to(reply_to.clone());
        }
        if config.transport == Transport::File {
            builder = builder.file_dir(config.file_dir.clone());
        }
        if config.transport == Transport::Smtp {
            builder = builder.smtp(config.smtp.clone());
        }
        builder.build()
    }

    /// Build a mailer from any custom transport.
    #[must_use]
    pub fn with_transport(transport: impl MailTransport + 'static) -> Self {
        Self {
            defaults: Arc::new(MailerDefaults::default()),
            transport: Arc::new(transport),
            delivery_queue: None,
        }
    }

    /// Attach a durable [`MailDeliveryQueue`] used by [`Self::deliver_later`].
    #[must_use]
    pub fn with_delivery_queue(mut self, queue: impl MailDeliveryQueue + 'static) -> Self {
        self.delivery_queue = Some(Arc::new(queue));
        self
    }

    /// Returns whether a durable [`MailDeliveryQueue`] is attached.
    #[must_use]
    pub fn has_durable_delivery_queue(&self) -> bool {
        self.delivery_queue.is_some()
    }

    /// Returns `true` when the active transport is intentionally a no-op
    /// (i.e. `transport = "disabled"` in `autumn.toml`).
    ///
    /// Handlers that require mail (e.g. forgot-password) can guard against
    /// silently dropped messages by checking this before attempting to send.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.transport.is_disabled()
    }

    /// Send mail immediately.
    ///
    /// # Errors
    ///
    /// Returns an error from the selected transport.
    pub async fn send(&self, mail: Mail) -> Result<(), MailError> {
        self.transport
            .send(mail.with_defaults(&self.defaults))
            .await
    }

    /// Queue mail for later delivery.
    ///
    /// When called **inside a [`Db::tx`](autumn_web::db::Db::tx) block**, the
    /// delivery is automatically deferred until the transaction commits. On
    /// rollback the mail is silently dropped — no orphaned sends.
    ///
    /// This deferral is process-local. It prevents mail for rolled-back writes,
    /// but it does not make the post-commit mail handoff crash-safe unless the
    /// configured [`MailDeliveryQueue`] records a durable outbox/queue entry.
    ///
    /// When called outside any active transaction the behaviour is unchanged:
    /// the mail is dispatched in a background Tokio task immediately.
    ///
    /// Use [`deliver_later_eager`](Self::deliver_later_eager) when you need the
    /// mail to fire regardless of whether the surrounding transaction commits
    /// (e.g. security alerts that must go out on any code path).
    pub fn deliver_later(&self, mail: Mail) {
        if let Err(error) = self.try_deliver_later(mail) {
            tracing::error!(error = %error, "background mail delivery was not scheduled");
        }
    }

    /// Queue mail for later delivery, **bypassing any active transaction**.
    ///
    /// Unlike [`deliver_later`](Self::deliver_later), this method always
    /// spawns the delivery immediately — it does not check for an active
    /// `db.tx` block. Use this when the mail must be sent even if the
    /// surrounding transaction rolls back (e.g. "someone tried to log in"
    /// security alerts, rate-limit notices).
    pub fn deliver_later_eager(&self, mail: Mail) {
        if let Err(error) = self.try_deliver_later_eager(mail) {
            tracing::error!(error = %error, "background mail delivery was not scheduled");
        }
    }

    /// Queue mail for later delivery, deferring when inside a `db.tx`.
    ///
    /// # Errors
    ///
    /// Returns an error when no active Tokio runtime is available to host the
    /// background task.
    ///
    /// # Panics
    ///
    /// Panics if the internal after-commit registry mutex is poisoned.
    pub fn try_deliver_later(&self, mail: Mail) -> Result<(), MailError> {
        if self.transport.is_disabled() {
            return Ok(());
        }
        let mail = mail.with_defaults(&self.defaults);

        // When inside a db.tx, push the spawn as an after-commit callback so
        // the mail only fires if the transaction commits successfully.
        #[cfg(feature = "db")]
        {
            let mailer = self.clone();
            let deferred = mail.clone();
            let mut f_opt: Option<(Self, Mail)> = Some((mailer, deferred));
            // Capture the caller's span now; the after-commit callback runs in a
            // fresh task with no request span, so spawn_mail_delivery would see an
            // empty span and lose trace correlation without this.
            let deliver_span = tracing::Span::current();

            crate::db::AFTER_COMMIT_REGISTRY
                .try_with(|registry| {
                    let (m, m_mail) = f_opt.take().expect("once");
                    let span = deliver_span.clone();
                    let boxed: crate::db::CommitCallback = Box::new(move || {
                        Box::pin(tracing::Instrument::instrument(
                            async move {
                                if let Some(queue) = m.delivery_queue.clone() {
                                    queue.enqueue(m_mail).await.map_err(|e| {
                                        crate::AutumnError::internal_server_error_msg(e.to_string())
                                    })
                                } else {
                                    m.spawn_mail_delivery(m_mail).map_err(|e| {
                                        crate::AutumnError::internal_server_error_msg(e.to_string())
                                    })
                                }
                            },
                            span,
                        ))
                    });
                    registry.lock().expect("registry lock").push(boxed);
                })
                .ok();

            if f_opt.is_none() {
                // Successfully registered for after-commit; skip the eager spawn.
                return Ok(());
            }
        }

        // Outside a transaction (or `db` feature not enabled) — spawn immediately.
        self.spawn_mail_delivery(mail)
    }

    /// Queue mail for later delivery, always spawning immediately.
    ///
    /// # Errors
    ///
    /// Returns an error when no active Tokio runtime is available.
    pub fn try_deliver_later_eager(&self, mail: Mail) -> Result<(), MailError> {
        if self.transport.is_disabled() {
            return Ok(());
        }
        let mail = mail.with_defaults(&self.defaults);
        self.spawn_mail_delivery(mail)
    }

    fn spawn_mail_delivery(&self, mail: Mail) -> Result<(), MailError> {
        // Honor the disabled-transport contract: if the operator turned mail off
        // for this profile, deliver_later must drop the message just like
        // immediate `send` does — even when a queue is attached.
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            MailError::RuntimeUnavailable(
                "deliver_later requires an active Tokio runtime".to_owned(),
            )
        })?;
        let parent_span = tracing::Span::current();
        if let Some(queue) = self.delivery_queue.clone() {
            handle.spawn(tracing::Instrument::instrument(
                async move {
                    if let Err(error) = queue.enqueue(mail).await {
                        tracing::error!(error = %error, "durable mail enqueue failed");
                    }
                },
                parent_span,
            ));
        } else {
            let mailer = self.clone();
            handle.spawn(tracing::Instrument::instrument(
                async move {
                    if let Err(error) = mailer.send(mail).await {
                        tracing::error!(error = %error, "background mail delivery failed");
                    }
                },
                parent_span,
            ));
        }
        Ok(())
    }
}

impl FromRequestParts<AppState> for Mailer {
    type Rejection = AutumnError;

    async fn from_request_parts(
        _parts: &mut http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        state
            .extension::<Self>()
            .as_deref()
            .cloned()
            .ok_or_else(|| AutumnError::service_unavailable_msg("Mailer is not configured"))
    }
}

/// Builder for [`Mailer`].
#[derive(Clone)]
pub struct MailerBuilder {
    transport: Transport,
    from: Option<String>,
    reply_to: Option<String>,
    file_dir: PathBuf,
    smtp: Option<SmtpConfig>,
    delivery_queue: Option<Arc<dyn MailDeliveryQueue>>,
}

impl Default for MailerBuilder {
    fn default() -> Self {
        Self {
            transport: Transport::Log,
            from: None,
            reply_to: None,
            file_dir: default_file_dir(),
            smtp: None,
            delivery_queue: None,
        }
    }
}

impl MailerBuilder {
    /// Select the transport.
    #[must_use]
    pub const fn transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// Set default From header.
    #[must_use]
    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    /// Set default Reply-To header.
    #[must_use]
    pub fn reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// Set file output directory.
    #[must_use]
    pub fn file_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.file_dir = dir.as_ref().to_path_buf();
        self
    }

    /// Set SMTP config.
    #[must_use]
    pub fn smtp(mut self, smtp: SmtpConfig) -> Self {
        self.smtp = Some(smtp);
        self
    }

    /// Attach a durable [`MailDeliveryQueue`] used by
    /// [`Mailer::deliver_later`].
    #[must_use]
    pub fn delivery_queue(mut self, queue: impl MailDeliveryQueue + 'static) -> Self {
        self.delivery_queue = Some(Arc::new(queue));
        self
    }

    /// Attach an already-shared durable [`MailDeliveryQueue`].
    #[must_use]
    pub fn delivery_queue_arc(mut self, queue: Arc<dyn MailDeliveryQueue>) -> Self {
        self.delivery_queue = Some(queue);
        self
    }

    /// Build the mailer.
    ///
    /// # Errors
    ///
    /// Returns an error when the SMTP transport or default addresses cannot be configured.
    pub fn build(self) -> Result<Mailer, MailError> {
        if let Some(from) = &self.from {
            parse_mailbox(from)?;
        }
        if let Some(reply_to) = &self.reply_to {
            parse_mailbox(reply_to)?;
        }

        let transport: Arc<dyn MailTransport> = match self.transport {
            Transport::Log => Arc::new(LogTransport),
            Transport::File => Arc::new(FileTransport { dir: self.file_dir }),
            Transport::Disabled => Arc::new(DisabledTransport),
            Transport::Smtp => Arc::new(SmtpTransport::new(self.smtp.unwrap_or_default())?),
        };

        Ok(Mailer {
            defaults: Arc::new(MailerDefaults {
                from: self.from,
                reply_to: self.reply_to,
            }),
            transport,
            delivery_queue: self.delivery_queue,
        })
    }
}

struct DisabledTransport;

impl MailTransport for DisabledTransport {
    fn send<'a>(
        &'a self,
        _mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn is_disabled(&self) -> bool {
        true
    }
}

struct LogTransport;

impl MailTransport for LogTransport {
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async move {
            tracing::info!(
                from = ?mail.from,
                reply_to = ?mail.reply_to,
                to = ?mail.to,
                subject = %mail.subject,
                text = ?mail.text,
                html = ?mail.html,
                "mail captured by log transport"
            );
            Ok(())
        })
    }
}

struct FileTransport {
    dir: PathBuf,
}

static FILE_TRANSPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl MailTransport for FileTransport {
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(&self.dir).await?;
            let filename = file_transport_filename(&mail);
            let path = self.dir.join(filename);
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .await?;
            let eml = render_eml(&mail);
            tokio::io::AsyncWriteExt::write_all(&mut file, eml.as_bytes()).await?;
            tokio::io::AsyncWriteExt::flush(&mut file).await?;
            file.sync_all().await?;
            Ok(())
        })
    }
}

struct SmtpTransport {
    inner: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpTransport {
    fn new(config: SmtpConfig) -> Result<Self, MailError> {
        let host = config
            .host
            .filter(|host| !host.trim().is_empty())
            .ok_or_else(|| MailError::InvalidMessage("mail.smtp.host is required".to_owned()))?;
        let mut builder = match config.tls {
            TlsMode::Disabled => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host),
            TlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?,
            TlsMode::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)?,
        };
        if let Some(port) = config.port {
            builder = builder.port(port);
        }
        if let Some(username) = config.username {
            let password_env = config.password_env.ok_or_else(|| {
                MailError::InvalidMessage(
                    "mail.smtp.password_env is required when mail.smtp.username is set".to_owned(),
                )
            })?;
            let password = std::env::var(&password_env).map_err(|_| {
                MailError::InvalidMessage("mail.smtp.password_env could not be resolved".to_owned())
            })?;
            builder = builder.credentials(Credentials::new(username, password));
        }
        Ok(Self {
            inner: builder.build(),
        })
    }
}

impl MailTransport for SmtpTransport {
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async move {
            let message = lettre_message(&mail)?;
            self.inner.send(message).await?;
            Ok(())
        })
    }
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn file_transport_filename(mail: &Mail) -> String {
    let sequence = FILE_TRANSPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}-{}-{:016x}-{}.eml",
        chrono::Utc::now().format("%Y%m%d%H%M%S%6f"),
        std::process::id(),
        sequence,
        sanitize_filename(mail.to.first().map_or("unknown", String::as_str))
    )
}

fn render_eml(mail: &Mail) -> String {
    let mut out = String::new();
    if let Some(from) = &mail.from {
        out.push_str("From: ");
        out.push_str(from);
        out.push('\n');
    }
    for to in &mail.to {
        out.push_str("To: ");
        out.push_str(to);
        out.push('\n');
    }
    if let Some(reply_to) = &mail.reply_to {
        out.push_str("Reply-To: ");
        out.push_str(reply_to);
        out.push('\n');
    }
    out.push_str("Date: ");
    out.push_str(&chrono::Utc::now().to_rfc2822());
    out.push('\n');
    out.push_str("Message-Id: <");
    out.push_str(&uuid::Uuid::new_v4().to_string());
    out.push_str("@autumn.local>\n");
    out.push_str("Subject: ");
    out.push_str(&mail.subject);
    out.push_str("\nMIME-Version: 1.0\n");
    if mail.html.is_some() && mail.text.is_some() {
        out.push_str("Content-Type: multipart/alternative; boundary=\"autumn-mail\"\n\n");
        if let Some(text) = &mail.text {
            out.push_str("--autumn-mail\nContent-Type: text/plain; charset=utf-8\n\n");
            out.push_str(text);
            out.push('\n');
        }
        if let Some(html) = &mail.html {
            out.push_str("--autumn-mail\nContent-Type: text/html; charset=utf-8\n\n");
            out.push_str(html);
            out.push('\n');
        }
        out.push_str("--autumn-mail--\n");
    } else if let Some(html) = &mail.html {
        out.push_str("Content-Type: text/html; charset=utf-8\n\n");
        out.push_str(html);
        out.push('\n');
    } else if let Some(text) = &mail.text {
        out.push_str("Content-Type: text/plain; charset=utf-8\n\n");
        out.push_str(text);
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone)]
struct ParsedMail {
    headers: Vec<(String, String)>,
    to: Vec<String>,
    subject: String,
    date: Option<String>,
    html: Option<String>,
    text: Option<String>,
    raw: String,
}

impl ParsedMail {
    fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
struct CapturedMailSummary {
    id: String,
    to: Vec<String>,
    subject: String,
    timestamp: String,
    modified: SystemTime,
}

pub(crate) fn mail_preview_router<S>(file_dir: PathBuf) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    AppState: axum::extract::FromRef<S>,
{
    let file_dir = Arc::new(file_dir);
    axum::Router::new()
        .route(
            MAIL_PREVIEW_PATH,
            axum::routing::get({
                let file_dir = Arc::clone(&file_dir);
                move |axum::extract::State(state): axum::extract::State<AppState>| {
                    let file_dir = Arc::clone(&file_dir);
                    async move { list_mail_preview(file_dir, state).await }
                }
            }),
        )
        .route(
            MAIL_PREVIEW_MESSAGE_PATH,
            axum::routing::get({
                let file_dir = Arc::clone(&file_dir);
                move |axum::extract::Path(message_id): axum::extract::Path<String>| {
                    let file_dir = Arc::clone(&file_dir);
                    async move { show_captured_mail(file_dir, message_id).await }
                }
            }),
        )
        .route(
            MAIL_PREVIEW_TEMPLATE_PATH,
            axum::routing::get(
                |axum::extract::Path((mailer, method)): axum::extract::Path<(String, String)>,
                 axum::extract::State(state): axum::extract::State<AppState>| async move {
                    show_template_preview(&state, &mailer, &method)
                },
            ),
        )
}

async fn list_mail_preview(file_dir: Arc<PathBuf>, state: AppState) -> Response {
    match captured_messages(&file_dir).await {
        Ok(messages) => {
            let previews = state
                .extension::<MailPreviewRegistry>()
                .map(|registry| registry.previews().to_vec())
                .unwrap_or_default();
            html_response(render_mail_index(&messages, &previews, &file_dir))
        }
        Err(error) => preview_error_response(&error),
    }
}

async fn show_captured_mail(file_dir: Arc<PathBuf>, message_id: String) -> Response {
    match read_captured_message(&file_dir, &message_id).await {
        Ok(parsed) => html_response(render_mail_detail(&parsed, "Captured message")),
        Err(error) => preview_error_response(&error),
    }
}

fn show_template_preview(state: &AppState, mailer: &str, method: &str) -> Response {
    let preview = state
        .extension::<MailPreviewRegistry>()
        .and_then(|registry| registry.find(mailer, method));
    let Some(preview) = preview else {
        return preview_error_response(&MailPreviewError::NotFound(format!("{mailer}/{method}")));
    };

    match preview.render() {
        Ok(mail) => {
            let raw = render_eml(&mail);
            let parsed = parse_eml(&raw);
            html_response(render_mail_detail(&parsed, "Template preview"))
        }
        Err(error) => preview_error_response(&error),
    }
}

async fn captured_messages(dir: &Path) -> Result<Vec<CapturedMailSummary>, MailPreviewError> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    let mut messages = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("eml"))
        {
            continue;
        }
        let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let metadata = entry.metadata().await?;
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        let raw = tokio::fs::read_to_string(&path).await?;
        let parsed = parse_eml(&raw);
        messages.push(CapturedMailSummary {
            id: id.to_owned(),
            to: parsed.to,
            subject: parsed.subject,
            timestamp: parsed.date.unwrap_or_else(|| format_system_time(modified)),
            modified,
        });
    }

    messages.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(messages)
}

async fn read_captured_message(
    dir: &Path,
    message_id: &str,
) -> Result<ParsedMail, MailPreviewError> {
    if !valid_message_id(message_id) {
        return Err(MailPreviewError::InvalidMessageId(message_id.to_owned()));
    }
    let path = dir.join(message_id);
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(MailPreviewError::NotFound(message_id.to_owned()));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(parse_eml(&raw))
}

fn valid_message_id(message_id: &str) -> bool {
    !message_id.is_empty()
        && Path::new(message_id)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("eml"))
        && !message_id.contains('/')
        && !message_id.contains('\\')
        && !message_id.contains("..")
}

fn parse_eml(raw: &str) -> ParsedMail {
    let normalized = raw.replace("\r\n", "\n");
    let (headers, body) = split_headers_body(&normalized);
    let content_type = header_value(&headers, "Content-Type").unwrap_or_default();
    let (html, text) = parse_mail_body(&content_type, body);
    let to = header_values(&headers, "To");
    let subject = header_value(&headers, "Subject").unwrap_or_else(|| "(no subject)".to_owned());
    let date = header_value(&headers, "Date");

    ParsedMail {
        headers,
        to,
        subject,
        date,
        html,
        text,
        raw: raw.to_owned(),
    }
}

fn split_headers_body(raw: &str) -> (Vec<(String, String)>, &str) {
    let Some((header_block, body)) = raw.split_once("\n\n") else {
        return (parse_header_block(raw), "");
    };
    (parse_header_block(header_block), body)
}

fn parse_header_block(header_block: &str) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in header_block.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, value)) = current.as_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some(header) = current.take() {
            headers.push(header);
        }
        if let Some((name, value)) = line.split_once(':') {
            current = Some((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    if let Some(header) = current {
        headers.push(header);
    }
    headers
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn header_values(headers: &[(String, String)], name: &str) -> Vec<String> {
    headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
        .collect()
}

fn parse_mail_body(content_type: &str, body: &str) -> (Option<String>, Option<String>) {
    if content_type
        .to_ascii_lowercase()
        .contains("multipart/alternative")
        && let Some(boundary) = content_type_boundary(content_type)
    {
        return parse_multipart_alternative(body, &boundary);
    }

    if content_type.to_ascii_lowercase().contains("text/html") {
        (Some(trim_body(body)), None)
    } else {
        (None, Some(trim_body(body)))
    }
}

fn parse_multipart_alternative(body: &str, boundary: &str) -> (Option<String>, Option<String>) {
    let marker = format!("--{boundary}");
    let mut html = None;
    let mut text = None;

    for segment in body.split(&marker).skip(1) {
        let segment = segment.trim_start_matches(['\n', '\r']);
        if segment.starts_with("--") {
            break;
        }
        let (headers, part_body) = split_headers_body(segment);
        let content_type = header_value(&headers, "Content-Type").unwrap_or_default();
        if content_type.to_ascii_lowercase().contains("text/html") {
            html = Some(trim_body(part_body));
        } else if content_type.to_ascii_lowercase().contains("text/plain") {
            text = Some(trim_body(part_body));
        }
    }

    (html, text)
}

fn content_type_boundary(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|part| {
        let part = part.trim();
        let (name, value) = part.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("boundary") {
            return None;
        }
        Some(value.trim().trim_matches('"').to_owned())
    })
}

fn trim_body(body: &str) -> String {
    body.trim_matches(['\r', '\n']).to_owned()
}

fn format_system_time(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn render_mail_index(
    messages: &[CapturedMailSummary],
    previews: &[MailPreview],
    file_dir: &Path,
) -> String {
    let mut body = String::new();
    body.push_str("<h1>Autumn Mail</h1>");
    body.push_str("<section><h2>Captured messages</h2>");
    if messages.is_empty() {
        body.push_str("<p class=\"empty\">No captured emails yet. Set <code>mail.transport = &quot;file&quot;</code>, send an email, then refresh this page. Autumn reads <code>");
        body.push_str(&escape_html(&file_dir.display().to_string()));
        body.push_str("</code>.</p>");
    } else {
        body.push_str(
            "<table><thead><tr><th>Timestamp</th><th>To</th><th>Subject</th></tr></thead><tbody>",
        );
        for message in messages {
            body.push_str("<tr><td>");
            body.push_str(&escape_html(&message.timestamp));
            body.push_str("</td><td>");
            body.push_str(&escape_html(&message.to.join(", ")));
            body.push_str("</td><td><a href=\"");
            body.push_str(MAIL_PREVIEW_PATH);
            body.push_str("/messages/");
            body.push_str(&escape_html(&message.id));
            body.push_str("\">");
            body.push_str(&escape_html(&message.subject));
            body.push_str("</a></td></tr>");
        }
        body.push_str("</tbody></table>");
    }
    body.push_str("</section><section><h2>Template previews</h2>");
    if previews.is_empty() {
        body.push_str("<p class=\"empty\">No mailer previews registered.</p>");
    } else {
        body.push_str("<table><thead><tr><th>Mailer</th><th>Preview</th></tr></thead><tbody>");
        for preview in previews {
            body.push_str("<tr><td>");
            body.push_str(&escape_html(preview.mailer()));
            body.push_str("</td><td><a href=\"");
            body.push_str(MAIL_PREVIEW_PATH);
            body.push_str("/previews/");
            body.push_str(&escape_html(preview.mailer()));
            body.push('/');
            body.push_str(&escape_html(preview.method()));
            body.push_str("\">");
            body.push_str(&escape_html(preview.method()));
            body.push_str("</a></td></tr>");
        }
        body.push_str("</tbody></table>");
    }
    body.push_str("</section>");
    render_mail_preview_layout("Autumn Mail", &body)
}

fn render_mail_detail(parsed: &ParsedMail, label: &str) -> String {
    let mut body = String::new();
    body.push_str("<p><a href=\"");
    body.push_str(MAIL_PREVIEW_PATH);
    body.push_str("\">Back to mail</a></p><h1>");
    body.push_str(&escape_html(&parsed.subject));
    body.push_str("</h1><p class=\"muted\">");
    body.push_str(&escape_html(label));
    body.push_str("</p>");

    if let Some(html) = &parsed.html {
        body.push_str("<iframe title=\"Rendered HTML email\" sandbox srcdoc=\"");
        body.push_str(&escape_html(html));
        body.push_str("\"></iframe>");
    } else {
        body.push_str("<p class=\"empty\">No HTML body was found for this email.</p>");
    }

    body.push_str("<details><summary>Plain text</summary><pre>");
    body.push_str(&escape_html(parsed.text.as_deref().unwrap_or("")));
    body.push_str("</pre></details>");

    body.push_str("<details><summary>Headers</summary><dl>");
    for header in ["From", "To", "Reply-To", "Subject", "Date", "Message-Id"] {
        if let Some(value) = parsed.header_value(header) {
            body.push_str("<dt>");
            body.push_str(header);
            body.push_str("</dt><dd>");
            body.push_str(&escape_html(value));
            body.push_str("</dd>");
        }
    }
    body.push_str("</dl></details>");

    body.push_str("<details><summary>Raw .eml</summary><pre>");
    body.push_str(&escape_html(&parsed.raw));
    body.push_str("</pre></details>");

    render_mail_preview_layout(&parsed.subject, &body)
}

fn render_mail_preview_layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{}</style></head><body>{}</body></html>",
        escape_html(title),
        MAIL_PREVIEW_CSS,
        body
    )
}

const MAIL_PREVIEW_CSS: &str = r#"
body{margin:0;padding:24px;font-family:system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color:#1f2933;background:#f6f8fa}
h1{margin:0 0 16px;font-size:28px}
h2{margin:28px 0 12px;font-size:18px}
table{width:100%;border-collapse:collapse;background:white;border:1px solid #d9e2ec}
th,td{padding:10px 12px;border-bottom:1px solid #e5eaf0;text-align:left;font-size:14px;vertical-align:top}
th{background:#edf2f7;color:#394b59;font-weight:650}
a{color:#0b63ce;text-decoration:none}
a:hover{text-decoration:underline}
.empty,.muted{color:#52616f}
code,pre{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}
pre{white-space:pre-wrap;background:#111827;color:#f8fafc;padding:12px;overflow:auto}
iframe{width:100%;min-height:420px;border:1px solid #cbd5e1;background:white}
details{margin-top:14px;background:white;border:1px solid #d9e2ec;padding:10px 12px}
summary{cursor:pointer;font-weight:650}
dt{font-weight:650;margin-top:8px}
dd{margin:2px 0 8px}
"#;

fn html_response(html: String) -> Response {
    Html(html).into_response()
}

fn preview_error_response(error: &MailPreviewError) -> Response {
    let status = match error {
        MailPreviewError::NotFound(_) | MailPreviewError::InvalidMessageId(_) => {
            http::StatusCode::NOT_FOUND
        }
        MailPreviewError::Io(_) | MailPreviewError::PreviewPanicked { .. } => {
            http::StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Html(render_mail_preview_layout(
            "Mail preview error",
            &format!(
                "<h1>Mail preview error</h1><p>{}</p>",
                escape_html(&error.to_string())
            ),
        )),
    )
        .into_response()
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn parse_mailbox(address: &str) -> Result<Mailbox, MailError> {
    address.parse().map_err(|source| MailError::InvalidAddress {
        address: address.to_owned(),
        source,
    })
}

fn lettre_message(mail: &Mail) -> Result<Message, MailError> {
    let from = mail
        .from
        .as_deref()
        .ok_or_else(|| MailError::InvalidMessage("mail from address is required".to_owned()))?;
    let mut builder = Message::builder().from(parse_mailbox(from)?);
    for to in &mail.to {
        builder = builder.to(parse_mailbox(to)?);
    }
    if let Some(reply_to) = &mail.reply_to {
        builder = builder.reply_to(parse_mailbox(reply_to)?);
    }
    builder = builder.subject(mail.subject.clone());

    match (&mail.text, &mail.html) {
        (Some(text), Some(html)) => Ok(builder.multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(text.clone()))
                .singlepart(SinglePart::html(html.clone())),
        )?),
        (Some(text), None) => Ok(builder.singlepart(SinglePart::plain(text.clone()))?),
        (None, Some(html)) => Ok(builder.singlepart(SinglePart::html(html.clone()))?),
        (None, None) => Err(MailError::InvalidMessage(
            "mail must include html or text body".to_owned(),
        )),
    }
}

struct InterceptedMailTransport {
    inner: Arc<dyn MailTransport>,
    interceptor: Arc<dyn crate::interceptor::MailInterceptor>,
}

impl MailTransport for InterceptedMailTransport {
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async move {
            let inner = Arc::clone(&self.inner);
            let mail_for_next = mail.clone();
            let next = Box::pin(async move { inner.send(mail_for_next).await });
            self.interceptor.intercept(&mail, next).await
        })
    }

    fn is_disabled(&self) -> bool {
        self.inner.is_disabled()
    }
}

/// Install the configured mailer into app state.
///
/// Picks up a runtime-installed [`MailDeliveryQueueHandle`] from
/// [`AppState`] extensions when present, so plugins (Harvest, Redis-backed,
/// etc.) can register durable delivery before this runs. In `prod` with a
/// non-`Disabled` transport, startup fails when neither a durable queue nor
/// [`MailConfig::allow_in_process_deliver_later_in_production`] is set, unless
/// `enforce_durable_guard` is `false` (used by short-lived contexts like
/// static-site builds where `deliver_later` semantics don't apply).
///
/// # Errors
///
/// Returns an Autumn error when the configured transport cannot be created or
/// when the production `deliver_later` guard is not satisfied.
pub(crate) fn install_mailer(
    state: &AppState,
    config: &MailConfig,
    enforce_durable_guard: bool,
) -> AutumnResult<()> {
    let mut mailer = Mailer::from_config(config).map_err(AutumnError::service_unavailable)?;

    if let Some(interceptor) = state.extension::<Arc<dyn crate::interceptor::MailInterceptor>>() {
        mailer.transport = Arc::new(InterceptedMailTransport {
            inner: Arc::clone(&mailer.transport),
            interceptor: (*interceptor).clone(),
        });
    }

    let in_production = matches!(state.profile(), "prod" | "production");
    let transport_sends_mail = config.transport != Transport::Disabled;

    // Honor the disabled transport contract: if the operator turned mail off
    // for this profile (tests, review apps, etc.), `deliver_later` must also
    // be a no-op — even when a durable queue was registered globally.
    if transport_sends_mail {
        let queue_handle = state.extension::<MailDeliveryQueueHandle>();
        if let Some(handle) = queue_handle.as_ref() {
            mailer.delivery_queue = Some(Arc::clone(handle.inner()));
        }
    }

    if enforce_durable_guard && in_production && transport_sends_mail {
        let has_durable_queue = mailer.delivery_queue.is_some();
        if !has_durable_queue && !config.allow_in_process_deliver_later_in_production {
            return Err(AutumnError::service_unavailable_msg(
                "mail.deliver_later has no durable backend in prod: register a MailDeliveryQueueHandle on AppState or set mail.allow_in_process_deliver_later_in_production = true to opt into the in-process Tokio fallback",
            ));
        }
        if !has_durable_queue {
            tracing::warn!(
                "mail.deliver_later is using the in-process Tokio fallback in prod; this is acknowledged via mail.allow_in_process_deliver_later_in_production but is not durable across restarts or replicas"
            );
        }
    }

    state.insert_extension(mailer);
    Ok(())
}

/// Run the optional [`MailDeliveryQueue`] factory and install the configured
/// mailer.
///
/// Centralizes the wiring used by every [`AppBuilder`](crate::app::AppBuilder)
/// build path: optionally invoke `queue_factory` against the live `AppState`,
/// register the resulting [`MailDeliveryQueueHandle`], then call
/// [`install_mailer`]. The factory is skipped entirely when
/// `enforce_durable_guard` is `false` (static-site builds), since the queue
/// may capture infrastructure (Redis, Harvest, etc.) that isn't available in
/// the asset-build environment.
///
/// # Errors
///
/// Propagates errors from the queue factory and from [`install_mailer`].
pub(crate) fn install_mailer_with_factory<F>(
    state: &AppState,
    config: &MailConfig,
    queue_factory: Option<F>,
    enforce_durable_guard: bool,
) -> AutumnResult<()>
where
    F: FnOnce(&AppState) -> AutumnResult<Arc<dyn MailDeliveryQueue>>,
{
    // Honor the disabled transport contract: a profile that turned mail off
    // (tests, review apps, etc.) must not open queue infrastructure either,
    // since all sends — immediate and deferred — are supposed to be no-ops.
    let transport_sends_mail = config.transport != Transport::Disabled;
    if enforce_durable_guard
        && transport_sends_mail
        && let Some(factory) = queue_factory
    {
        let queue = factory(state)?;
        state.insert_extension(MailDeliveryQueueHandle::from_arc(queue));
    }
    install_mailer(state, config, enforce_durable_guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_builder_rejects_missing_body() {
        let err = Mail::builder()
            .to("user@example.com")
            .subject("Hello")
            .build()
            .expect_err("body should be required");
        assert!(err.to_string().contains("html or text"));
    }

    #[test]
    fn filename_sanitizer_keeps_safe_characters() {
        assert_eq!(
            sanitize_filename("Ada Lovelace <ada@example.com>"),
            "Ada_Lovelace__ada_example.com_"
        );
    }

    #[test]
    fn transport_default_is_disabled() {
        assert_eq!(Transport::default(), Transport::Disabled);
    }

    #[test]
    fn smtp_config_validation_rejects_whitespace_only_host() {
        let config = MailConfig {
            transport: Transport::Smtp,
            smtp: SmtpConfig {
                host: Some("   ".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };

        let error = config
            .validate(Some("dev"))
            .expect_err("whitespace SMTP host should be rejected");

        assert!(error.to_string().contains("mail.smtp.host is required"));
    }

    #[test]
    fn transport_env_value_is_trimmed_and_case_insensitive() {
        assert_eq!(Transport::from_env_value(" SMTP "), Some(Transport::Smtp));
        assert_eq!(Transport::from_env_value(" LoG "), Some(Transport::Log));
    }

    #[test]
    fn tls_mode_env_value_is_trimmed_and_case_insensitive() {
        assert_eq!(TlsMode::from_env_value(" TLS "), Some(TlsMode::Tls));
        assert_eq!(
            TlsMode::from_env_value(" START_TLS "),
            Some(TlsMode::StartTls)
        );
        assert_eq!(
            TlsMode::from_env_value(" disabled "),
            Some(TlsMode::Disabled)
        );
    }

    #[test]
    fn file_transport_filename_is_unique_for_same_recipient() {
        let mail = Mail::builder()
            .to("Ada Lovelace <ada@example.com>")
            .subject("Hello")
            .text("body")
            .build()
            .expect("mail should build");

        let first = file_transport_filename(&mail);
        let second = file_transport_filename(&mail);

        assert_ne!(first, second);
        assert!(
            Path::new(&first)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("eml"))
        );
        assert!(
            Path::new(&second)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("eml"))
        );
    }

    #[test]
    fn smtp_transport_rejects_missing_password_env_when_username_is_set() {
        // Eris regression test: the original password environment variable name should not be logged.
        // It's checked here as a proxy for the value, as the actual failure is when the env lookup fails.
        let missing_key = format!(
            "AUTUMN_TEST_MISSING_SMTP_PASSWORD_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let Err(error) = SmtpTransport::new(SmtpConfig {
            host: Some("smtp.example.com".to_owned()),
            port: Some(587),
            username: Some("mailer".to_owned()),
            password_env: Some(missing_key.clone()),
            tls: TlsMode::StartTls,
        }) else {
            panic!("missing password env should fail at startup");
        };

        let err_string = error.to_string();
        assert!(err_string.contains("mail.smtp.password_env could not be resolved"));
        assert!(!err_string.contains(&missing_key));
    }

    #[test]
    fn smtp_transport_rejects_missing_password_env_key_when_username_is_set() {
        let Err(error) = SmtpTransport::new(SmtpConfig {
            host: Some("smtp.example.com".to_owned()),
            port: Some(587),
            username: Some("mailer".to_owned()),
            password_env: None,
            tls: TlsMode::StartTls,
        }) else {
            panic!("missing password_env setting should fail at startup");
        };

        assert!(error.to_string().contains("mail.smtp.password_env"));
    }

    #[test]
    fn mailer_builder_rejects_invalid_default_from_address() {
        let Err(error) = Mailer::builder().from("not an email address").build() else {
            panic!("invalid default from should fail fast");
        };

        match error {
            MailError::InvalidAddress { address, .. } => {
                assert_eq!(address, "not an email address");
            }
            other => panic!("expected invalid address error, got {other:?}"),
        }
    }

    #[test]
    fn mailer_from_config_rejects_invalid_default_reply_to_address() {
        let config = MailConfig {
            transport: Transport::Smtp,
            from: Some("Autumn <noreply@example.com>".to_owned()),
            reply_to: Some("definitely not an address".to_owned()),
            smtp: SmtpConfig {
                host: Some("smtp.example.com".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };

        let Err(error) = Mailer::from_config(&config) else {
            panic!("invalid configured reply-to should fail at construction");
        };

        match error {
            MailError::InvalidAddress { address, .. } => {
                assert_eq!(address, "definitely not an address");
            }
            other => panic!("expected invalid address error, got {other:?}"),
        }
    }

    #[test]
    fn try_deliver_later_returns_error_without_runtime() {
        let mailer = Mailer::builder().build().expect("mailer should build");
        let mail = Mail::builder()
            .to("user@example.com")
            .subject("Hello")
            .text("hello")
            .build()
            .expect("mail should build");

        let error = mailer
            .try_deliver_later(mail)
            .expect_err("missing runtime should return an error");

        assert!(error.to_string().contains("active Tokio runtime"));
    }

    #[test]
    fn deliver_later_does_not_panic_without_runtime() {
        let mailer = Mailer::builder().build().expect("mailer should build");
        let mail = Mail::builder()
            .to("user@example.com")
            .subject("Hello")
            .text("hello")
            .build()
            .expect("mail should build");

        mailer.deliver_later(mail);
    }

    fn sample_smtp_config() -> MailConfig {
        MailConfig {
            transport: Transport::Smtp,
            from: Some("Autumn <noreply@example.com>".to_owned()),
            smtp: SmtpConfig {
                host: Some("smtp.example.com".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn sample_mail() -> Mail {
        Mail::builder()
            .to("user@example.com")
            .subject("Hi")
            .text("hello")
            .build()
            .expect("mail should build")
    }

    struct NoopQueue;

    impl MailDeliveryQueue for NoopQueue {
        fn enqueue<'a>(
            &'a self,
            _mail: Mail,
        ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn install_mailer_rejects_in_process_fallback_in_prod_without_ack() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();

        let error = install_mailer(&state, &config, true)
            .expect_err("prod must reject in-process deliver_later fallback without ack");

        let message = error.to_string();
        assert!(
            message.contains("allow_in_process_deliver_later_in_production"),
            "error should explain how to opt in: {message}"
        );
    }

    #[test]
    fn install_mailer_allows_in_process_fallback_in_prod_with_explicit_ack() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = MailConfig {
            allow_in_process_deliver_later_in_production: true,
            ..sample_smtp_config()
        };

        install_mailer(&state, &config, true).expect("explicit ack should permit fallback in prod");
    }

    #[test]
    fn install_mailer_allows_durable_queue_in_prod_without_ack() {
        let state = crate::AppState::for_test().with_profile("prod");
        state.insert_extension(MailDeliveryQueueHandle::new(NoopQueue));
        let config = sample_smtp_config();

        install_mailer(&state, &config, true)
            .expect("a registered durable queue should satisfy the prod guard");
    }

    #[test]
    fn install_mailer_does_not_require_ack_outside_production() {
        let state = crate::AppState::for_test().with_profile("dev");
        let config = sample_smtp_config();

        install_mailer(&state, &config, true).expect("non-prod profiles should not require an ack");
    }

    #[test]
    fn install_mailer_does_not_require_ack_when_transport_is_disabled() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = MailConfig::default();

        install_mailer(&state, &config, true)
            .expect("disabled transport never sends mail so it should not need an ack");
    }

    struct CapturingQueue {
        tx: tokio::sync::mpsc::UnboundedSender<Mail>,
    }

    impl MailDeliveryQueue for CapturingQueue {
        fn enqueue<'a>(
            &'a self,
            mail: Mail,
        ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
            let tx = self.tx.clone();
            Box::pin(async move {
                tx.send(mail)
                    .map_err(|err| MailError::RuntimeUnavailable(err.to_string()))?;
                Ok(())
            })
        }
    }

    #[cfg(feature = "db")]
    struct FailingQueue {
        tx: tokio::sync::mpsc::UnboundedSender<Mail>,
    }

    #[cfg(feature = "db")]
    impl MailDeliveryQueue for FailingQueue {
        fn enqueue<'a>(
            &'a self,
            mail: Mail,
        ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
            let tx = self.tx.clone();
            Box::pin(async move {
                tx.send(mail)
                    .map_err(|err| MailError::RuntimeUnavailable(err.to_string()))?;
                Err(MailError::RuntimeUnavailable("queue offline".to_owned()))
            })
        }
    }

    #[cfg(feature = "db")]
    async fn drain_after_commit_callbacks_for_test(
        registry: &std::sync::Arc<std::sync::Mutex<Vec<crate::db::CommitCallback>>>,
    ) {
        let callbacks: Vec<crate::db::CommitCallback> = {
            let mut reg = registry.lock().expect("registry lock");
            std::mem::take(&mut *reg)
        };

        for cb in callbacks {
            if let Err(error) = cb().await {
                crate::db::record_after_commit_failure();
                tracing::error!("test drain: after_commit callback failed: {error}");
            }
        }
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn deferred_deliver_later_queue_failure_increments_after_commit_counter() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Mail>();
        let mailer = Mailer::builder()
            .delivery_queue(FailingQueue { tx })
            .build()
            .expect("mailer should build");
        let registry = std::sync::Arc::new(std::sync::Mutex::new(
            Vec::<crate::db::CommitCallback>::new(),
        ));
        let before =
            crate::db::AFTER_COMMIT_FAILURES_TOTAL.load(std::sync::atomic::Ordering::Relaxed);

        crate::db::AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                mailer
                    .try_deliver_later(sample_mail())
                    .expect("registering deferred mail should succeed");
            })
            .await;

        drain_after_commit_callbacks_for_test(&registry).await;

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("queue should be called within 1s")
            .expect("queue should receive the mail");
        assert_eq!(received.subject, "Hi");

        let after =
            crate::db::AFTER_COMMIT_FAILURES_TOTAL.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            after > before,
            "deferred durable mail handoff failures should count as after_commit failures"
        );
    }

    #[tokio::test]
    async fn deliver_later_routes_through_configured_queue() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Mail>();

        let mailer = Mailer::builder()
            .delivery_queue(CapturingQueue { tx })
            .build()
            .expect("mailer should build");

        mailer
            .try_deliver_later(sample_mail())
            .expect("scheduling onto the queue should succeed");

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("queue should receive within 1s")
            .expect("queue should receive the mail");

        assert_eq!(received.subject, "Hi");
    }

    #[tokio::test]
    async fn deliver_later_without_queue_sends_via_transport_directly() {
        // When no delivery queue is configured, `spawn_mail_delivery` falls back to
        // calling `mailer.send()` in a background task.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TrackingSend(Arc<AtomicBool>);
        impl MailTransport for TrackingSend {
            fn send<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                self.0.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }
        }

        let sent = Arc::new(AtomicBool::new(false));
        let mailer = Mailer::with_transport(TrackingSend(sent.clone()));

        mailer
            .try_deliver_later(sample_mail())
            .expect("should succeed without queue");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            sent.load(Ordering::SeqCst),
            "mail should have been sent directly via transport"
        );
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn deferred_deliver_later_without_queue_sends_after_commit() {
        // After-commit callback with no queue falls back to `spawn_mail_delivery`
        // which calls `mailer.send()` in a spawned task.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TrackingSend(Arc<AtomicBool>);
        impl MailTransport for TrackingSend {
            fn send<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                self.0.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }
        }

        let sent = Arc::new(AtomicBool::new(false));
        let mailer = Mailer::with_transport(TrackingSend(sent.clone()));
        let registry = std::sync::Arc::new(std::sync::Mutex::new(
            Vec::<crate::db::CommitCallback>::new(),
        ));

        crate::db::AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                mailer
                    .try_deliver_later(sample_mail())
                    .expect("should succeed");
            })
            .await;

        drain_after_commit_callbacks_for_test(&registry).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(
            sent.load(Ordering::SeqCst),
            "mail should have been sent after commit via direct transport"
        );
    }

    #[tokio::test]
    async fn mailer_with_transport_starts_without_delivery_queue() {
        let mailer = Mailer::with_transport(NoopTransport);
        assert!(
            !mailer.has_durable_delivery_queue(),
            "with_transport should default to no durable queue"
        );
        // Exercise NoopTransport::send so its body is also covered.
        mailer
            .send(sample_mail())
            .await
            .expect("noop transport should always succeed");
    }

    struct NoopTransport;
    impl MailTransport for NoopTransport {
        fn send<'a>(
            &'a self,
            _mail: Mail,
        ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn deliver_later_is_noop_when_transport_disabled_even_with_queue() {
        // The Mailer-level builder lets callers attach a queue *and* pick
        // Transport::Disabled. The disabled-transport contract requires
        // deliver_later to drop the message in that case — the queue must
        // not persist mail when the operator has turned mail off entirely.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Mail>();
        let mailer = Mailer::builder()
            .transport(Transport::Disabled)
            .delivery_queue(CapturingQueue { tx })
            .build()
            .expect("mailer should build");

        mailer
            .try_deliver_later(sample_mail())
            .expect("disabled transport should succeed as a no-op");

        // Wait briefly for any spawn that might erroneously fire to land.
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(
            received.is_err(),
            "queue must not be invoked when transport is disabled"
        );
    }

    #[tokio::test]
    async fn deliver_later_uses_in_process_fallback_when_no_queue() {
        // The default Mailer has no durable queue, so deliver_later should
        // still spawn the in-process Tokio task and not call any queue.
        let mailer = Mailer::builder().build().expect("mailer should build");

        mailer
            .try_deliver_later(sample_mail())
            .expect("in-process fallback should still schedule");
    }

    #[test]
    fn mail_delivery_queue_handle_round_trips_via_from_arc_and_inner() {
        let arc: Arc<dyn MailDeliveryQueue> = Arc::new(NoopQueue);
        let handle = MailDeliveryQueueHandle::from_arc(Arc::clone(&arc));

        assert!(Arc::ptr_eq(handle.inner(), &arc));
    }

    #[test]
    fn mail_delivery_queue_handle_debug_does_not_panic() {
        let handle = MailDeliveryQueueHandle::new(NoopQueue);
        let rendered = format!("{handle:?}");
        assert!(rendered.contains("MailDeliveryQueueHandle"));
    }

    #[test]
    fn mailer_has_durable_delivery_queue_reflects_attachment() {
        let plain = Mailer::builder().build().expect("mailer should build");
        assert!(!plain.has_durable_delivery_queue());

        let with_queue = Mailer::builder()
            .delivery_queue(NoopQueue)
            .build()
            .expect("mailer should build");
        assert!(with_queue.has_durable_delivery_queue());
    }

    #[test]
    fn mailer_with_delivery_queue_post_build_attaches_queue() {
        let mailer = Mailer::builder()
            .build()
            .expect("mailer should build")
            .with_delivery_queue(NoopQueue);

        assert!(mailer.has_durable_delivery_queue());
    }

    #[test]
    fn mailer_builder_delivery_queue_arc_attaches_shared_queue() {
        let arc: Arc<dyn MailDeliveryQueue> = Arc::new(NoopQueue);
        let mailer = Mailer::builder()
            .delivery_queue_arc(arc)
            .build()
            .expect("mailer should build");

        assert!(mailer.has_durable_delivery_queue());
    }

    #[test]
    fn install_mailer_warns_but_succeeds_with_explicit_ack_in_prod() {
        // Same as the explicit-ack test, but also asserts the mailer was
        // actually inserted and has no durable queue attached.
        let state = crate::AppState::for_test().with_profile("prod");
        let config = MailConfig {
            allow_in_process_deliver_later_in_production: true,
            ..sample_smtp_config()
        };

        install_mailer(&state, &config, true).expect("explicit ack should permit fallback in prod");

        let installed = state
            .extension::<Mailer>()
            .expect("install_mailer should store a Mailer extension");
        assert!(
            !installed.has_durable_delivery_queue(),
            "no queue was registered, so installed mailer should fall back in-process"
        );
    }

    #[test]
    fn install_mailer_attaches_registered_queue_to_mailer() {
        let state = crate::AppState::for_test().with_profile("prod");
        state.insert_extension(MailDeliveryQueueHandle::new(NoopQueue));
        let config = sample_smtp_config();

        install_mailer(&state, &config, true).expect("durable queue should permit prod startup");

        let installed = state
            .extension::<Mailer>()
            .expect("install_mailer should store a Mailer extension");
        assert!(
            installed.has_durable_delivery_queue(),
            "registered queue handle should be attached to the installed mailer"
        );
    }

    #[test]
    fn install_mailer_with_factory_runs_factory_and_attaches_queue() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();
        let factory_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let captured = Arc::clone(&factory_called);

        let factory = move |_state: &crate::AppState| {
            captured.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, crate::AutumnError>(Arc::new(NoopQueue) as Arc<dyn MailDeliveryQueue>)
        };

        install_mailer_with_factory(&state, &config, Some(factory), true)
            .expect("factory should produce a queue and satisfy the prod guard");

        assert!(
            factory_called.load(std::sync::atomic::Ordering::SeqCst),
            "factory must run when enforce_durable_guard is true"
        );
        let installed = state
            .extension::<Mailer>()
            .expect("install_mailer should store a Mailer extension");
        assert!(
            installed.has_durable_delivery_queue(),
            "factory's queue should be wired into the installed Mailer"
        );
    }

    #[test]
    fn install_mailer_with_factory_skips_factory_when_not_enforced() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();
        let factory_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let captured = Arc::clone(&factory_called);

        let factory = move |_state: &crate::AppState| {
            captured.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, crate::AutumnError>(Arc::new(NoopQueue) as Arc<dyn MailDeliveryQueue>)
        };

        install_mailer_with_factory(&state, &config, Some(factory), false)
            .expect("static-build path should skip factory and install cleanly");

        assert!(
            !factory_called.load(std::sync::atomic::Ordering::SeqCst),
            "factory must be skipped when enforce_durable_guard is false"
        );
    }

    #[test]
    fn install_mailer_with_factory_propagates_factory_errors() {
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();

        let factory = |_state: &crate::AppState| {
            Err::<Arc<dyn MailDeliveryQueue>, _>(crate::AutumnError::service_unavailable_msg(
                "queue offline",
            ))
        };

        let error = install_mailer_with_factory(&state, &config, Some(factory), true)
            .expect_err("factory error should propagate");
        assert!(error.to_string().contains("queue offline"));
    }

    #[test]
    fn install_mailer_with_factory_skips_factory_when_transport_disabled() {
        // Even when enforce_durable_guard=true (normal server path), a
        // profile with transport=disabled must not run the factory: the
        // factory might open Redis/Harvest/DB connections, but all mail in
        // this profile is supposed to be a no-op.
        let state = crate::AppState::for_test().with_profile("dev");
        let config = MailConfig::default(); // transport = Disabled
        let factory_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let captured = Arc::clone(&factory_called);

        let factory = move |_state: &crate::AppState| {
            captured.store(true, std::sync::atomic::Ordering::SeqCst);
            Err::<Arc<dyn MailDeliveryQueue>, _>(crate::AutumnError::service_unavailable_msg(
                "queue must not be reached",
            ))
        };

        install_mailer_with_factory(&state, &config, Some(factory), true)
            .expect("disabled transport should bypass the factory entirely");
        assert!(
            !factory_called.load(std::sync::atomic::Ordering::SeqCst),
            "factory must not run when transport = disabled"
        );
    }

    #[test]
    fn install_mailer_with_factory_works_without_factory() {
        type FactoryFn = fn(&crate::AppState) -> AutumnResult<Arc<dyn MailDeliveryQueue>>;
        let state = crate::AppState::for_test().with_profile("dev");
        let config = sample_smtp_config();
        let no_factory: Option<FactoryFn> = None;

        install_mailer_with_factory(&state, &config, no_factory, true)
            .expect("absent factory should be fine in non-prod");
    }

    #[test]
    fn install_mailer_does_not_run_factory_when_not_enforced_and_no_handle() {
        // Mirrors run_build_mode: queue factory is intentionally skipped, so
        // no MailDeliveryQueueHandle is on AppState. install_mailer must
        // tolerate this and not try to enforce or warn about a missing queue.
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();

        install_mailer(&state, &config, false)
            .expect("static-build mode should install cleanly with no queue handle");

        let installed = state
            .extension::<Mailer>()
            .expect("install_mailer should store a Mailer extension");
        assert!(
            !installed.has_durable_delivery_queue(),
            "no queue is expected when run_build_mode skips the factory"
        );
    }

    #[test]
    fn install_mailer_skips_production_guard_when_not_enforced() {
        // Static-site builds (run_build_mode) call install_mailer with
        // enforce_durable_guard=false because they don't run the request
        // loop and don't actually defer mail. Even with a prod profile,
        // an active SMTP transport, no queue, and no ack flag, install
        // must succeed in this mode.
        let state = crate::AppState::for_test().with_profile("prod");
        let config = sample_smtp_config();

        install_mailer(&state, &config, false)
            .expect("static-build mode should not enforce the deliver_later guard");
    }

    #[test]
    fn spawn_mail_delivery_inherits_parent_span() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::{Arc, Mutex};

        struct CapturingQueue(Arc<Mutex<Option<tracing::span::Id>>>);
        impl MailDeliveryQueue for CapturingQueue {
            fn enqueue<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                let captured = self.0.clone();
                Box::pin(async move {
                    *captured.lock().unwrap() = tracing::Span::current().id();
                    Ok(())
                })
            }
        }

        let captured_span_id: Arc<Mutex<Option<tracing::span::Id>>> = Arc::new(Mutex::new(None));

        let mailer = Mailer::builder()
            .delivery_queue(CapturingQueue(captured_span_id.clone()))
            .build()
            .expect("mailer with queue should build");
        let mail = sample_mail();

        // The subscriber must remain active for the entire duration — spanning
        // both the enqueue call and the spawned task's execution — so that
        // `tracing::Span::current()` inside the task sees the same span tree
        // that was active when `try_deliver_later` was called.
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");

            let outer = tracing::info_span!("deliver_later_outer");
            let outer_id = outer.id();

            rt.block_on(async {
                {
                    let _guard = outer.enter();
                    mailer
                        .try_deliver_later(mail)
                        .expect("deliver_later must not fail");
                }

                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            });

            let in_task = captured_span_id.lock().unwrap().clone();
            assert_eq!(
                in_task, outer_id,
                "delivery task must run inside the span that called deliver_later"
            );
        });
    }

    #[tokio::test]
    async fn spawn_mail_delivery_logs_error_when_queue_fails() {
        use std::future::Future;
        use std::pin::Pin;

        struct AlwaysFailQueue;
        impl MailDeliveryQueue for AlwaysFailQueue {
            fn enqueue<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                Box::pin(async { Err(MailError::RuntimeUnavailable("always fails".to_owned())) })
            }
        }

        let mailer = Mailer::builder()
            .delivery_queue(AlwaysFailQueue)
            .build()
            .expect("build");

        mailer
            .try_deliver_later(sample_mail())
            .expect("should schedule");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn spawn_mail_delivery_logs_error_when_transport_fails() {
        use std::future::Future;
        use std::pin::Pin;

        struct AlwaysFailTransport;
        impl MailTransport for AlwaysFailTransport {
            fn send<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                Box::pin(async {
                    Err(MailError::RuntimeUnavailable(
                        "transport offline".to_owned(),
                    ))
                })
            }
        }

        let mailer = Mailer::with_transport(AlwaysFailTransport);

        mailer
            .try_deliver_later(sample_mail())
            .expect("should schedule");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[test]
    fn install_mailer_does_not_attach_queue_when_transport_disabled() {
        // When mail.transport = "disabled" the operator has explicitly turned
        // mail off for this profile (tests, review apps, etc.). A globally
        // registered queue must not turn deliver_later back into a durable
        // persist; it should remain a no-op.
        let state = crate::AppState::for_test().with_profile("dev");
        state.insert_extension(MailDeliveryQueueHandle::new(NoopQueue));
        let config = MailConfig::default(); // transport = Disabled

        install_mailer(&state, &config, true).expect("disabled transport should install cleanly");

        let installed = state
            .extension::<Mailer>()
            .expect("install_mailer should store a Mailer extension");
        assert!(
            !installed.has_durable_delivery_queue(),
            "disabled transport must suppress queue attachment so deliver_later is a no-op"
        );
    }

    #[tokio::test]
    async fn intercepted_mail_transport_short_circuit_prevents_sync_execution() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicU32, Ordering};

        static TRANSPORT_CALLS: AtomicU32 = AtomicU32::new(0);

        struct CountingTransport;
        impl MailTransport for CountingTransport {
            fn send<'a>(
                &'a self,
                _mail: Mail,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                TRANSPORT_CALLS.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(()) })
            }

            fn is_disabled(&self) -> bool {
                false
            }
        }

        struct ShortCircuitMailInterceptor;
        impl crate::interceptor::MailInterceptor for ShortCircuitMailInterceptor {
            fn intercept<'a>(
                &'a self,
                _mail: &'a Mail,
                _next: Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>,
            ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
                Box::pin(async move {
                    Err(MailError::RuntimeUnavailable(
                        "blocked by interceptor".to_owned(),
                    ))
                })
            }
        }

        let transport = Arc::new(CountingTransport);
        let interceptor = Arc::new(ShortCircuitMailInterceptor);
        let intercepted = InterceptedMailTransport {
            inner: transport,
            interceptor,
        };

        let mail = Mail::builder()
            .to("test@example.com")
            .subject("test")
            .text("body")
            .build()
            .unwrap();

        TRANSPORT_CALLS.store(0, Ordering::SeqCst);

        let res = intercepted.send(mail).await;
        assert!(res.is_err());
        assert_eq!(TRANSPORT_CALLS.load(Ordering::SeqCst), 0);
    }
}
