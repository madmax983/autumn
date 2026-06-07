# Bot Protection & CAPTCHA

Autumn's bot protection middleware validates a CAPTCHA token on every
`application/x-www-form-urlencoded` POST/PUT/PATCH/DELETE request before it
reaches a handler.  Requests that fail the challenge receive a
`400 Bad Request` [Problem Details](https://www.rfc-editor.org/rfc/rfc9457)
response; handlers never see them.

Two production providers are built in — Cloudflare Turnstile and hCaptcha —
and both a dev-mode bypass and a deterministic test provider ship for local
development and automated tests.

---

## Quick start

### 1. Configure `autumn.toml`

```toml
[bot_protection]
enabled    = true
provider   = "turnstile"          # "turnstile" (default) or "hcaptcha"
site_key   = "0x4AAAA..."        # rendered into the widget; safe to commit
secret_key = "..."                # server-side secret — use an env var!
dev_bypass = false
```

Never commit `secret_key`.  Set it via the environment:

```sh
export AUTUMN_BOT_PROTECTION__SECRET_KEY="your-secret"
```

### 2. Add the widget to your form

```rust,no_run
use autumn_web::prelude::*;

#[get("/signup")]
async fn signup_form(config: AutumnConfig) -> Markup {
    html! {
        form method="POST" action="/signup" {
            input type="email" name="email";
            (bot_protection_widget(&config.bot_protection))
            button { "Sign up" }
        }
    }
}
```

`bot_protection_widget` renders the provider-appropriate `<div>` placeholder
and `<script>` tag.  You do not need to add the script tag yourself.

### 3. Handlers stay clean

The middleware runs before your handler.  If the CAPTCHA passes, the request
arrives at the handler body-intact; if it fails, your handler is never called.

```rust,no_run
#[post("/signup")]
async fn signup_submit(form: Form<SignupForm>) -> impl IntoResponse {
    // Only reached when CAPTCHA verification succeeded.
    "Welcome!"
}
```

---

## Configuration reference

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Enable the middleware. |
| `provider` | `"turnstile"` \| `"hcaptcha"` | `"turnstile"` | CAPTCHA backend. |
| `site_key` | string | — | Public widget key (rendered to the browser). |
| `secret_key` | string | — | Private server-side verification secret. |
| `form_field` | string | provider default | Override the form field name scanned for the token. |
| `dev_bypass` | bool | `false` | Skip verification; any request passes. |

Provider defaults for `form_field`:

| Provider | Default field |
|----------|--------------|
| Turnstile | `cf-turnstile-response` |
| hCaptcha | `h-captcha-response` |

---

## Dev-mode bypass

In local development you generally do not want to complete a CAPTCHA on every
form submission.  Set `dev_bypass = true`:

```toml
# autumn.toml (dev profile)
[bot_protection]
enabled    = true
dev_bypass = true
```

With `dev_bypass = true`:

- The middleware is still wired in (so integration tests catch middleware ordering bugs).
- Verification is skipped; every request passes unconditionally.
- `bot_protection_widget` renders a hidden `<input>` instead of the real widget,
  so HTML forms submit without requiring user interaction.

A startup log line confirms bypass is active:

```
INFO  autumn_web::router: bot_protection provider=Turnstile dev_bypass=true
```

---

## Provider switching

Switch providers by changing the `provider` key — no code changes needed:

```toml
[bot_protection]
enabled    = true
provider   = "hcaptcha"
site_key   = "your-hcaptcha-site-key"
secret_key = "your-hcaptcha-secret"
```

Both providers expose the same middleware interface.  The widget helper
automatically renders the correct `<div>` class and `<script>` source.

---

## Test-mode bypass

For automated tests inject a deterministic provider instead of configuring
real provider credentials:

```rust,no_run
use std::sync::Arc;
use autumn_web::security::captcha::{BotProtectionLayer, TestCaptchaProvider};
use autumn_web::test::TestApp;

// Only "correct-token" passes; every other value yields 400.
let layer = BotProtectionLayer::new(Arc::new(TestCaptchaProvider::new("correct-token")));

let client = TestApp::new()
    .routes(routes![submit])
    .layer(layer)
    .build();

// Correct token → 200
client
    .post("/submit")
    .body("cf-turnstile-response=correct-token")
    .send()
    .await
    .assert_status(200);

// Wrong token → 400
client
    .post("/submit")
    .body("cf-turnstile-response=bad-token")
    .send()
    .await
    .assert_status(400);
```

Use [`AlwaysPassProvider`] when you want every request to pass without a token:

```rust,no_run
let layer = BotProtectionLayer::new(Arc::new(AlwaysPassProvider));
```

---

## Scope: only HTML forms are challenged

The middleware inspects only `application/x-www-form-urlencoded` requests.
Other content types — JSON API calls, multipart file uploads, and most external
webhooks — pass through without any CAPTCHA check.  This means:

- REST JSON endpoints are unaffected.
- File upload endpoints (`multipart/form-data`) are unaffected.
- Webhooks delivered as JSON (Stripe, GitHub, etc.) are unaffected.

Only classic HTML form submissions — the intended target — are verified.

### URL-encoded webhook payloads (Slack slash commands, etc.)

Some webhook senders — most notably Slack slash commands — deliver payloads as
`application/x-www-form-urlencoded`.  Those requests will be challenged by the
middleware and will fail because they cannot include a CAPTCHA token.

The recommended solution is to **not apply bot protection globally** when your
application handles url-encoded webhooks.  Instead, scope the middleware to only
the router that serves your public forms:

```rust,no_run
use autumn_web::prelude::*;
use autumn_web::security::captcha::{BotProtectionLayer, AlwaysPassProvider};
use std::sync::Arc;

// Public form routes — protected by CAPTCHA.
let forms_router = Router::new()
    .route("/signup", post(signup_submit))
    .layer(BotProtectionLayer::from_config(&config.bot_protection));

// Webhook routes — no CAPTCHA (signature verification happens inside the handler).
let webhook_router = Router::new()
    .route("/webhooks/slack", post(slack_handler));

let app = Router::new()
    .merge(forms_router)
    .merge(webhook_router);
```

With this layout the bot protection middleware is only applied to the routes that
need it and the webhook endpoints are left untouched.

---

## Edge cases

### No-JS clients

Cloudflare Turnstile supports an [invisible mode](https://developers.cloudflare.com/turnstile/get-started/client-side-rendering/#invisible-turnstile-widget)
that completes without any user gesture, making it compatible with most
no-JS-unfriendly clients at the server level.  However, the JavaScript widget
itself requires a browser to execute.

For clients that truly cannot run JavaScript (CLI tools, server-to-server
calls), bot protection applies only to form submissions.  API endpoints using
`application/json` are not challenged by this middleware.  If you need to
protect API endpoints, consider rate limiting (`[security.rate_limit]`) or
`Authorization` header validation instead.

### Accessibility

Both Turnstile and hCaptcha expose accessibility-friendly CAPTCHA modes:

- **Cloudflare Turnstile** is [designed to be invisible](https://developers.cloudflare.com/turnstile/) for most users and requires no puzzle-solving interaction for well-behaved browsers.  It is WCAG 2.1 compatible.
- **hCaptcha** offers an [accessibility cookie](https://www.hcaptcha.com/accessibility) that lets users with disabilities bypass challenges site-wide.

Neither provider requires users to identify objects in images by default in
their modern widget modes.

### Replay / double-submission protection

CAPTCHA tokens are single-use on the provider side.  Submitting the same token
twice will fail verification on the second attempt.  Ensure your forms do not
replay requests on network errors without generating a fresh token.

### Missing `secret_key`

If `enabled = true` and `dev_bypass = false` but `secret_key` is missing,
Autumn logs a warning at startup:

```
WARN  autumn_web::security::captcha: bot_protection: enabled is true and dev_bypass is false,
      but secret_key is missing or empty — all CAPTCHA verifications will fail!
```

Every mutating form request will then receive a `400 Bad Request`.  Always
supply `secret_key` in production or set `dev_bypass = true` in development.

---

## Custom providers

Implement [`CaptchaProvider`] to integrate any third-party CAPTCHA service:

```rust,no_run
use std::{future::Future, pin::Pin, sync::Arc};
use autumn_web::security::captcha::{BotProtectionLayer, CaptchaProvider};

struct MyProvider;

impl CaptchaProvider for MyProvider {
    fn verify<'a>(&'a self, token: &'a str) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            // Call your CAPTCHA service here.
            !token.is_empty()
        })
    }

    fn form_field_name(&self) -> &str {
        "my-captcha-token"
    }

    #[cfg(feature = "maud")]
    fn widget_markup(&self, site_key: &str) -> maud::Markup {
        maud::html! {
            div data-my-captcha=(site_key) {}
            script src="https://example.com/captcha.js" {}
        }
    }
}

let layer = BotProtectionLayer::new(Arc::new(MyProvider));
```

Pass the layer to `AppBuilder::layer` (or `TestApp::layer` in tests) and it
plugs straight into the middleware stack.
