# 🔭 Vantage: Spec for OAuth2 Support

**Epic:** EPIC-011 (v1.0 Security & Stability)
**Priority:** Should Have
**Story Points:** 8
**Status:** Not Started
**Assigned To:** markm
**Created:** 2026-04-17
**Sprint:** Backlog

---

## User Story

👤 **User Story:** As an Application Developer, I want to authenticate users using third-party providers (like Google, GitHub, or Okta) via OAuth2/OIDC, so that users can log in securely without creating new passwords, reducing onboarding friction and improving account security.

---

## The "So What?" Ask

**What business problem does this solve?**
Creating new accounts with passwords is a major source of friction for new users, leading to higher drop-off rates during onboarding. Furthermore, managing passwords increases the security burden on application developers. By providing built-in OAuth2/OIDC support, developers can seamlessly integrate "Log in with Google/GitHub" functionality, improving conversion rates and shifting the security burden of credential management to trusted identity providers. This capability is essential for modern web applications.

---

## Gap Analysis

**Look at the market:**
- **Spring Boot:** Offers Spring Security OAuth2, which seamlessly integrates with numerous providers and provides extensive configuration options via `application.properties`.
- **Loco / Other Rust Frameworks:** Often require manual integration with low-level crates like `oauth2` or `openidconnect`, leaving developers to wire up callbacks, session management, and CSRF state checks manually.
- **Our Gap:** Autumn currently relies on basic username/password authentication or custom session setups. There is no simple, framework-native way to configure OAuth2 providers or handle the resulting callbacks and token exchanges without writing significant boilerplate code.

---

## Acceptance Criteria

✅ **Acceptance Criteria:**
- Must support standard OAuth2 Authorization Code flow.
- Must support OpenID Connect (OIDC) for identity extraction.
- Must provide configuration primitives in `autumn.toml` (e.g., `[auth.oauth2.github] client_id=...`).
- Must provide a simple macro/extractor (e.g., `#[oauth2_callback]`) to handle the callback and extract user data securely.
- Must integrate seamlessly with existing session management to log the user in after successful authentication.
- Must handle state/nonce parameters automatically to prevent CSRF attacks during the OAuth flow.

---

## Metric Definition

Success = A developer can configure a "Log in with GitHub" button and callback handler in under 5 minutes, with less than 20 lines of Rust code, successfully extracting the user's identity into an Autumn session.

---

## Out of Scope

🚫 **Out of Scope:**
- Implementing custom identity providers.
- Supporting legacy OAuth 1.0a.
- Managing user profiles beyond initial authentication and identity extraction.
