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

use axum::extract::FromRequestParts;
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

        Ok(())
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
}

/// Durable backend for [`Mailer::deliver_later`].
///
/// Implementors persist the mail (DB row, Redis stream, Harvest job, etc.) and
/// return as soon as the handoff is durable. The framework's in-process Tokio
/// fallback is intentionally not durable; production deployments should
/// register a real implementation via [`MailDeliveryQueueHandle`] before
/// [`install_mailer`] runs, or set
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
/// Designed for storage on [`AppState`](crate::AppState) extensions. Plugins
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
    /// This release falls back to an in-process Tokio task. The method shape is
    /// intentionally stable so Harvest-backed durable dispatch can slot in
    /// behind the same call once the web crate and Harvest plugin share a
    /// first-class queue contract.
    pub fn deliver_later(&self, mail: Mail) {
        if let Err(error) = self.try_deliver_later(mail) {
            tracing::error!(error = %error, "background mail delivery was not scheduled");
        }
    }

    /// Queue mail for later delivery.
    ///
    /// # Errors
    ///
    /// Returns an error when no active Tokio runtime is available to host the
    /// background task.
    pub fn try_deliver_later(&self, mail: Mail) -> Result<(), MailError> {
        let mail = mail.with_defaults(&self.defaults);
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            MailError::RuntimeUnavailable(
                "deliver_later requires an active Tokio runtime".to_owned(),
            )
        })?;
        if let Some(queue) = self.delivery_queue.clone() {
            handle.spawn(async move {
                if let Err(error) = queue.enqueue(mail).await {
                    tracing::error!(error = %error, "durable mail enqueue failed");
                }
            });
        } else {
            let mailer = self.clone();
            handle.spawn(async move {
                if let Err(error) = mailer.send(mail).await {
                    tracing::error!(error = %error, "background mail delivery failed");
                }
            });
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
            let password = std::env::var(&password_env).map_err(|error| {
                MailError::InvalidMessage(format!(
                    "mail.smtp.password_env={password_env:?} could not be resolved: {error}"
                ))
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

        assert!(error.to_string().contains(&missing_key));
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
}
