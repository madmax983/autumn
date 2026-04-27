//! Transactional email support.
//!
//! The public surface is intentionally small: build a [`Mail`] value, send it
//! through the cloneable [`Mailer`] extractor, and swap transports through the
//! [`MailTransport`] trait when SMTP is not the right coffin lining.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

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
    #[default]
    Log,
    /// Write RFC 822 `.eml` files under `target/mail` or a configured dir.
    File,
    /// Send through SMTP using Lettre.
    Smtp,
    /// Drop all email sends successfully.
    Disabled,
}

impl Transport {
    pub(crate) fn from_env_value(value: &str) -> Option<Self> {
        match value {
            "log" | "Log" => Some(Self::Log),
            "file" | "File" => Some(Self::File),
            "smtp" | "Smtp" => Some(Self::Smtp),
            "disabled" | "Disabled" => Some(Self::Disabled),
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
        match value {
            "disabled" | "Disabled" => Some(Self::Disabled),
            "starttls" | "start_tls" | "StartTls" => Some(Self::StartTls),
            "tls" | "Tls" => Some(Self::Tls),
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

        if self.transport == Transport::Smtp && self.smtp.host.as_deref().unwrap_or("").is_empty() {
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
        }
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
        let mailer = self.clone();
        tokio::spawn(async move {
            if let Err(error) = mailer.send(mail).await {
                tracing::error!(error = %error, "background mail delivery failed");
            }
        });
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
}

impl Default for MailerBuilder {
    fn default() -> Self {
        Self {
            transport: Transport::Log,
            from: None,
            reply_to: None,
            file_dir: default_file_dir(),
            smtp: None,
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

    /// Build the mailer.
    ///
    /// # Errors
    ///
    /// Returns an error when the SMTP transport cannot be configured.
    pub fn build(self) -> Result<Mailer, MailError> {
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

impl MailTransport for FileTransport {
    fn send<'a>(
        &'a self,
        mail: Mail,
    ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(&self.dir).await?;
            let filename = format!(
                "{}-{}.eml",
                chrono::Utc::now().format("%Y%m%d%H%M%S%3f"),
                sanitize_filename(mail.to.first().map_or("unknown", String::as_str))
            );
            let path = self.dir.join(filename);
            tokio::fs::write(path, render_eml(&mail)).await?;
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
            let password = config
                .password_env
                .as_deref()
                .and_then(|key| std::env::var(key).ok())
                .unwrap_or_default();
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
/// # Errors
///
/// Returns an Autumn error when the configured transport cannot be created.
pub(crate) fn install_mailer(state: &AppState, config: &MailConfig) -> AutumnResult<()> {
    let mailer = Mailer::from_config(config).map_err(AutumnError::service_unavailable)?;
    if matches!(state.profile(), "prod" | "production") && config.transport != Transport::Disabled {
        tracing::warn!(
            "mail deliver_later currently uses the in-process Tokio fallback unless a Harvest-backed mail queue is installed"
        );
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
}
