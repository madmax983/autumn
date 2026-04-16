# S-059: OAuth2 / OIDC Support

## 👤 User Story
As an Application Developer, I want to authenticate users using third-party providers (like Google, GitHub, or Okta) via OAuth2/OIDC, so that users can log in securely without creating new passwords, reducing onboarding friction and improving account security.

## 💼 The "So What?" (Business Value)
- **Lower Barrier to Entry:** Users are more likely to sign up if they can use existing accounts.
- **Reduced Liability:** Delegating password management and 2FA to major identity providers reduces the security surface area.
- **Enterprise Readiness:** Support for OIDC allows B2B applications to integrate with enterprise SSO solutions.

## ✅ Acceptance Criteria
- Must support standard OAuth2 Authorization Code flow.
- Must support OpenID Connect (OIDC) for identity extraction.
- Must provide configuration primitives in `autumn.toml` (e.g., `[auth.oauth2.github] client_id=...`).
- Must provide a simple macro/extractor (e.g., `#[oauth2_callback]`) to handle the callback and extract user data securely.
- Must integrate seamlessly with existing session management to log the user in after successful authentication.
- Must handle state/nonce parameters automatically to prevent CSRF attacks during the OAuth flow.

## 🚫 Out of Scope
- Implementing custom identity providers.
- Supporting legacy OAuth 1.0a.
- Managing user profiles beyond initial authentication and identity extraction.

## 📊 Metrics
- Success = Developer can configure GitHub OAuth2 in < 5 minutes.
- Success = End-user login flow takes < 2 seconds from callback to authenticated session.

## 🔍 Gap Analysis
- Existing standard libraries (like `oauth2-rs` or `openidconnect-rs`) provide the low-level building blocks but require significant boilerplate to integrate with Axum/Tower and session management. Autumn should provide the Spring Boot-style ergonomic abstraction layer on top of these.
