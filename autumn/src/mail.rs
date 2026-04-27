use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use http::request::Parts;
use serde::Deserialize;

use crate::{AppState, AutumnError, AutumnResult};

type SendFuture = Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailTransport {
    Log,
    File,
    Smtp,
    Disabled,
}

impl Default for MailTransport {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurityMode {
    StartTls,
    Tls,
    None,
}

impl Default for SmtpSecurityMode {
    fn default() -> Self {
        Self::StartTls
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SmtpConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password_env: Option<String>,
    pub security: SmtpSecurityMode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MailConfig {
    #[serde(default)]
    pub transport: MailTransport,
    #[serde(default = "default_from")]
    pub from: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub allow_log_in_production: bool,
    #[serde(default = "default_file_output_dir")]
    pub file_output_dir: PathBuf,
    #[serde(default)]
    pub smtp: SmtpConfig,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            transport: MailTransport::Disabled,
            from: default_from(),
            reply_to: None,
            allow_log_in_production: false,
            file_output_dir: default_file_output_dir(),
            smtp: SmtpConfig::default(),
        }
    }
}

fn default_from() -> String {
    "no-reply@localhost".to_owned()
}

fn default_file_output_dir() -> PathBuf {
    PathBuf::from("target/mail")
}

impl MailConfig {
    pub fn validate(&self, profile: Option<&str>) -> Result<(), String> {
        let is_prod = profile.unwrap_or_default() == "prod";
        if is_prod && matches!(self.transport, MailTransport::Log) && !self.allow_log_in_production
        {
            return Err(
                "mail.transport=\"log\" is blocked in prod; set mail.allow_log_in_production=true to override".to_owned(),
            );
        }
        if matches!(self.transport, MailTransport::Smtp) && self.smtp.host.is_none() {
            return Err("mail.smtp.host is required when mail.transport=\"smtp\"".to_owned());
        }
        Ok(())
    }

    pub fn build_mailer(&self, profile: Option<&str>) -> AutumnResult<Mailer> {
        self.validate(profile)
            .map_err(AutumnError::service_unavailable_msg)?;
        let sender: Arc<dyn MailSender> = match self.transport {
            MailTransport::Disabled => Arc::new(DisabledSender),
            MailTransport::Log => Arc::new(LogSender),
            MailTransport::File => Arc::new(FileSender {
                base_dir: self.file_output_dir.clone(),
            }),
            MailTransport::Smtp => Arc::new(SmtpSender),
        };
        Ok(Mailer {
            sender,
            default_from: self.from.clone(),
            default_reply_to: self.reply_to.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Mail {
    pub to: Vec<String>,
    pub subject: String,
    pub html_body: String,
    pub text_body: Option<String>,
    pub from: Option<String>,
    pub reply_to: Option<String>,
}

impl Mail {
    #[must_use]
    pub fn new(
        to: impl Into<String>,
        subject: impl Into<String>,
        html_body: impl Into<String>,
    ) -> Self {
        Self {
            to: vec![to.into()],
            subject: subject.into(),
            html_body: html_body.into(),
            text_body: None,
            from: None,
            reply_to: None,
        }
    }

    #[must_use]
    pub fn with_text_body(mut self, text: impl Into<String>) -> Self {
        self.text_body = Some(text.into());
        self
    }
}

trait MailSender: Send + Sync {
    fn send(&self, mail: Mail) -> SendFuture;
}

#[derive(Clone)]
pub struct Mailer {
    sender: Arc<dyn MailSender>,
    default_from: String,
    default_reply_to: Option<String>,
}

impl Mailer {
    pub async fn send(&self, mut mail: Mail) -> AutumnResult<()> {
        if mail.from.is_none() {
            mail.from = Some(self.default_from.clone());
        }
        if mail.reply_to.is_none() {
            mail.reply_to = self.default_reply_to.clone();
        }
        self.sender.send(mail).await
    }

    pub async fn deliver_later(&self, mail: Mail) -> AutumnResult<()> {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(error) = this.send(mail).await {
                tracing::error!(error = %error, "async mail send failed");
            }
        });
        Ok(())
    }
}

impl<S> FromRequestParts<S> for Mailer
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AutumnError;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        app_state
            .extension::<Mailer>()
            .as_deref()
            .cloned()
            .ok_or_else(|| AutumnError::service_unavailable_msg("Mailer is not configured"))
    }
}

struct DisabledSender;

impl MailSender for DisabledSender {
    fn send(&self, _mail: Mail) -> SendFuture {
        Box::pin(async { Ok(()) })
    }
}

struct LogSender;

impl MailSender for LogSender {
    fn send(&self, mail: Mail) -> SendFuture {
        Box::pin(async move {
            tracing::info!(
                to = ?mail.to,
                subject = %mail.subject,
                from = ?mail.from,
                reply_to = ?mail.reply_to,
                html_body = %mail.html_body,
                text_body = ?mail.text_body,
                "mail delivered via log transport"
            );
            Ok(())
        })
    }
}

struct FileSender {
    base_dir: PathBuf,
}

impl MailSender for FileSender {
    fn send(&self, mail: Mail) -> SendFuture {
        let base = self.base_dir.clone();
        Box::pin(async move {
            tokio::fs::create_dir_all(&base)
                .await
                .map_err(AutumnError::internal_server_error)?;
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
            let to = mail.to.first().map_or("unknown".to_owned(), |value| {
                value.replace(['/', '\\', ':', '@'], "_")
            });
            let path = base.join(format!("{stamp}-{to}.eml"));
            let payload = format!(
                "From: {}\nTo: {}\nSubject: {}\n{}\nContent-Type: text/html; charset=utf-8\n\n{}\n",
                mail.from.unwrap_or_else(default_from),
                mail.to.join(", "),
                mail.subject,
                mail.reply_to
                    .map_or_else(String::new, |reply| format!("Reply-To: {reply}\n")),
                mail.html_body
            );
            tokio::fs::write(path, payload)
                .await
                .map_err(AutumnError::internal_server_error)?;
            Ok(())
        })
    }
}

struct SmtpSender;

impl MailSender for SmtpSender {
    fn send(&self, _mail: Mail) -> SendFuture {
        Box::pin(async {
            Err(AutumnError::service_unavailable_msg(
                "smtp transport is not available in this build yet",
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prod_rejects_log_transport_by_default() {
        let config = MailConfig {
            transport: MailTransport::Log,
            ..MailConfig::default()
        };
        let error = config
            .validate(Some("prod"))
            .expect_err("prod should block log transport by default");
        assert!(error.contains("allow_log_in_production"));
    }

    #[tokio::test]
    async fn file_transport_writes_eml_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = MailConfig {
            transport: MailTransport::File,
            file_output_dir: dir.path().to_path_buf(),
            ..MailConfig::default()
        };
        let mailer = config.build_mailer(Some("dev")).expect("build mailer");
        mailer
            .send(Mail::new("user@example.com", "hello", "<p>hi</p>"))
            .await
            .expect("send mail");

        let entries = std::fs::read_dir(dir.path())
            .expect("read dir")
            .collect::<Result<Vec<_>, _>>()
            .expect("entries");
        assert_eq!(entries.len(), 1);
    }
}
