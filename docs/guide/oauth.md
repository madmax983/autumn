# Sign in with OAuth2 / OIDC

Autumn supports first-class OAuth2 and OpenID Connect (OIDC) login via the `--oauth` flag on `autumn generate auth`.

## Quick start

```sh
autumn generate auth User --oauth github,google
```

This generates:

- `src/routes/oauth.rs` — redirect and callback handlers
- A migration for the `oauth_identities` table
- Login-page buttons for each listed provider
- `autumn.toml` stubs for `[auth.oauth2.github]` and `[auth.oauth2.google]`

## Provider presets

Autumn ships built-in presets for the three most common providers.  
Add the provider name to `autumn.toml` and fill in your credentials:

```toml
[auth.oauth2.github]
client_id     = "YOUR_CLIENT_ID"
client_secret = "YOUR_CLIENT_SECRET"   # use env var in production
redirect_uri  = "https://yourapp.com/auth/oauth/github/callback"
scope         = "read:user user:email"

[auth.oauth2.google]
client_id     = "YOUR_CLIENT_ID"
client_secret = "YOUR_CLIENT_SECRET"
redirect_uri  = "https://yourapp.com/auth/oauth/google/callback"
scope         = "openid email profile"

[auth.oauth2.microsoft]
client_id     = "YOUR_CLIENT_ID"
client_secret = "YOUR_CLIENT_SECRET"
redirect_uri  = "https://yourapp.com/auth/oauth/microsoft/callback"
scope         = "openid email profile"
```

| Provider  | Protocol | Discovery URL            | Notes                        |
|-----------|----------|--------------------------|------------------------------|
| Google    | OIDC     | accounts.google.com      | ID token validated via JWKS  |
| GitHub    | OAuth2   | n/a (explicit endpoints) | Uses `/user` and `/user/emails` userinfo |
| Microsoft | OIDC     | login.microsoftonline.com/common/v2.0 | Supports personal + work accounts |

## Registering redirect URIs

Each provider requires you to allowlist your callback URL before login works.

**GitHub** — Settings → Developer settings → OAuth Apps → New OAuth App  
Callback URL: `https://yourapp.com/auth/oauth/github/callback`

**Google** — Google Cloud Console → APIs & Services → Credentials → OAuth 2.0 Client  
Authorized redirect URI: `https://yourapp.com/auth/oauth/google/callback`

**Microsoft** — Azure portal → App registrations → Redirect URIs  
Platform: Web, URI: `https://yourapp.com/auth/oauth/microsoft/callback`

For local development use `http://localhost:3000/auth/oauth/<provider>/callback`.

## Security properties

### PKCE (S256)

Every authorization request uses PKCE with the S256 challenge method.  
The `code_verifier` (32 random bytes, base64url-encoded) is stored in the session; the `code_challenge = BASE64URL(SHA256(code_verifier))` is sent in the authorization URL.  
This prevents authorization code interception even if the callback URL is compromised.

### State anti-CSRF

A random `state` token is generated per-request and stored in the session.  
The callback handler validates it with a constant-time comparison (`subtle::ConstantTimeEq`).  
A mismatch returns a generic error — the offending value is never logged.

### Nonce (for OIDC)

When using OIDC providers (Google, Microsoft) a random `nonce` is embedded in the ID token and verified on the callback, preventing token-replay attacks.

## Account linking

By default (`create_account` policy) a new local user record is created automatically when an OAuth identity logs in for the first time.

To require existing accounts first, set in `autumn.toml`:

```toml
[auth]
oauth_linking_policy = "require_local_signup_first"
```

Under this policy, login with an unlinked provider identity returns an error directing the user to sign up locally first.

## Database schema

The generator creates an `oauth_identities` migration:

```sql
CREATE TABLE oauth_identities (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider   TEXT      NOT NULL,
    subject    TEXT      NOT NULL,   -- provider's stable user identifier
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (provider, subject)
);
```

One user can have multiple OAuth identities (e.g., sign in with both GitHub and Google).

## autumn doctor --strict

`autumn doctor --strict` fails when:

- `client_secret` is empty for any configured `[auth.oauth2.*]` provider **in production**

It warns (non-fatal) in development so the check still appears in the output.

Set secrets via environment variables to keep them out of `autumn.toml`:

```sh
export AUTUMN_AUTH__OAUTH2__GITHUB__CLIENT_SECRET="ghp_..."
export AUTUMN_AUTH__OAUTH2__GOOGLE__CLIENT_SECRET="..."
```

## Middleware

Generated callback routes are automatically opted into the CSRF, session, and security-headers middleware stacks.  No manual wiring is required.

## Opting out

The entire OAuth2 subsystem is gated behind the `oauth2` Cargo feature flag.  
If you never generate `--oauth` handlers the feature is never enabled and no OAuth2 code is compiled into your binary.
