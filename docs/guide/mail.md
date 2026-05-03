# Mail

Enable the optional mail subsystem when your app needs password resets, signup
confirmations, or transactional notifications:

```toml
autumn-web = { version = "0.3", features = ["mail"] }
```

## Configuration

Development profile defaults to log transport. Production refuses log transport
unless you explicitly acknowledge it.

```toml
[mail]
transport = "file" # log | file | smtp | disabled
from = "Acme <noreply@example.com>"
reply_to = "support@example.com"
file_dir = "target/mail"

[mail.smtp]
host = "smtp.example.com"
port = 587
username = "apikey"
password_env = "SMTP_PASSWORD"
tls = "starttls" # disabled | starttls | tls
```

Environment overrides use the same nested naming as the rest of Autumn:
`AUTUMN_MAIL__TRANSPORT`, `AUTUMN_MAIL__FROM`,
`AUTUMN_MAIL__SMTP__HOST`, `AUTUMN_MAIL__SMTP__PASSWORD_ENV`.

## Sending

`Mailer` is a cloneable extractor backed by app state:

```rust
use autumn_web::prelude::*;

#[post("/password-reset")]
async fn reset(mailer: Mailer) -> AutumnResult<&'static str> {
    let mail = Mail::builder()
        .to("user@example.com")
        .subject("Reset your password")
        .html(html! { p { "Use this reset link." } })
        .text("Use this reset link.")
        .build()?;

    mailer.send(mail).await?;
    Ok("sent")
}
```

## `#[mailer]`

Put templates on a small struct and let the macro generate `send_*` and
`deliver_later_*` helpers:

```rust
use autumn_web::prelude::*;

struct AccountMailer;

#[mailer]
impl AccountMailer {
    fn reset_password(&self, to: String, token: String) -> Mail {
        Mail::builder()
            .to(to)
            .subject("Reset your password")
            .html(html! { p { "Token: " (token) } })
            .text(format!("Token: {token}"))
            .build()
            .expect("static template should be valid")
    }
}
```

Call `AccountMailer.send_reset_password(&mailer, to, token).await` for an
immediate send. Call `deliver_later_reset_password` when the request should not
wait on SMTP.

If the route also persists DB state (for example, writing an outbox row plus
creating a user), wrap the DB side in [`Db::tx`](transactions.md) so your write
sequence is atomic.

## Transports

- `log`: writes headers and full bodies to tracing at INFO. Default for `dev`.
- `file`: writes `.eml` files under `target/mail` by default. This is ideal for
  integration tests and local inspection.
- `smtp`: sends through Lettre with rustls and Tokio.
- `disabled`: accepts sends and drops them.

For provider APIs like SES, Postmark, or SendGrid, implement `MailTransport` and
build a `Mailer::with_transport(...)`.

## Production Checklist

- Enable the `mail` feature.
- Use `transport = "smtp"` in `prod`.
- Keep SMTP secrets in environment variables via `password_env`.
- Add a plain-text fallback for every HTML email.
- Assert file-transport `.eml` contents in integration tests.
- Prefer a Harvest-backed queue for durable `deliver_later` retries. Without
  Harvest, Autumn falls back to an in-process Tokio task and logs failures.
- For DB-write + mail-orchestration flows, use the [Transactions
  Guide](transactions.md) for the canonical atomic write pattern.
