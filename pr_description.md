🔭 Vantage: Spec for OAuth2 Support

👤 **User Story:**
As an Application Developer, I want to authenticate users using third-party providers (like Google, GitHub, or Okta) via OAuth2/OIDC, so that users can log in securely without creating new passwords, reducing onboarding friction and improving account security.

✅ **Acceptance Criteria:**
- Must support standard OAuth2 Authorization Code flow.
- Must support OpenID Connect (OIDC) for identity extraction.
- Must provide configuration primitives in `autumn.toml` (e.g., `[auth.oauth2.github] client_id=...`).
- Must provide a simple macro/extractor (e.g., `#[oauth2_callback]`) to handle the callback and extract user data securely.
- Must integrate seamlessly with existing session management to log the user in after successful authentication.
- Must handle state/nonce parameters automatically to prevent CSRF attacks during the OAuth flow.

🚫 **Out of Scope:**
- Implementing custom identity providers.
- Supporting legacy OAuth 1.0a.
- Managing user profiles beyond initial authentication and identity extraction.
