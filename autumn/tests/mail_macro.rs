#![cfg(feature = "mail")]

use autumn_web::prelude::*;

struct AccountMailer;

#[mailer]
impl AccountMailer {
    fn reset_password(&self, to: String, token: String) -> Mail {
        let _ = std::mem::size_of_val(self);
        Mail::builder()
            .to(to)
            .subject("Reset your password")
            .html(html! { p { "Token: " (token) } })
            .text(token)
            .build()
            .expect("valid mail")
    }
}

#[test]
fn mailer_macro_generates_send_helpers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mailer = Mailer::builder()
        .from("noreply@example.com")
        .transport(Transport::File)
        .file_dir(dir.path())
        .build()
        .expect("mailer should build");

    let account = AccountMailer;
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime
        .block_on(account.send_reset_password(
            &mailer,
            "user@example.com".to_owned(),
            "abc123".to_owned(),
        ))
        .expect("send helper should work");

    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("mail dir exists")
            .count(),
        1
    );
}
