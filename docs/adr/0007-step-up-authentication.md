# ADR 0007: Step-Up Authentication ("Sudo Mode")

- Status: Accepted
- Date: 2026-06-06
- Deciders: Autumn maintainers
- Tags: auth, security, session, middleware, macros

## Context

Autumn's `#[secured]` attribute ensures a handler only runs for authenticated
users. It does not constrain *when* the user authenticated. A session that is
hours or days old can exercise any `#[secured]` route, meaning a stolen cookie,
an XSS-exfiltrated session, or an unattended logged-in browser exposes the full
destructive surface of the application to an attacker.

Sensitive operations — account deletion, MFA removal, email change, credential
export — require a stricter guarantee: the user must have re-proven their
identity *recently* (within a short, configurable window). Every mature
framework addresses this pattern eventually:

- GitHub's explicit "sudo mode" re-prompts for credentials before
  privileged API calls and sensitive settings changes.
- Django's `django.contrib.admin` re-prompts before admin login.
- Rails apps hand-roll a `before_action :require_recent_authentication!` with
  a `session[:last_authenticated_at]` check.

Without a first-class primitive, every autumn application reinvents this
pattern differently — or not at all. This becomes a SOC 2 / HIPAA blocker when
auditors ask how account deletion is gated.

## Decision

Autumn ships a first-class step-up authentication primitive consisting of:

### 1. Session Claim

`last_strong_auth_at` — a Unix timestamp (stored as a string in the session
KV store alongside the existing `user_id` / auth key). Set on initial login
and refreshed when the user completes a reauth challenge. No schema migration
is needed; it lives entirely in the session payload.

### 2. `#[step_up]` Proc Macro

A route attribute macro in `autumn-macros` that injects a compile-time
freshness check before the handler body runs. The generated code:

1. Reads `last_strong_auth_at` from the session.
2. Compares it against `now() - max_age_secs`.
3. If stale or absent:
   - **Browser clients**: redirect to `/reauth?return_to=<path>`.
   - **JSON clients** (`Accept: application/json`): return
     `401 application/problem+json` with `WWW-Authenticate: StepUp max-age=N`.
4. Emits `auth.step_up.success` or `auth.step_up.failure` audit events.

### 3. Configuration Hierarchy

```
per-route  #[step_up(max_age = "10m")]     ← highest priority
global     [auth.step_up] default_max_age_secs = 300  ← via autumn.toml
fallback   DEFAULT_MAX_AGE_SECS = 300      ← compiled-in constant
```

### 4. Same-Origin `return_to` Validation

`return_to` is validated to reject open-redirect attacks. A value is accepted
only when it starts with `/` and does not start with `//`.

### 5. Scaffolds

The auth starter scaffold pre-wires `#[step_up]` on:
- `account_destroy` (POST /account/destroy) — irreversible account deletion.
- `two_factor_disable` (POST /account/2fa/disable) — MFA removal.

### 6. Admin Plugin Flag

`AdminPlugin::with_step_up_mutations()` adds a Tower middleware layer that
guards all `POST | PUT | PATCH | DELETE` requests in the admin router.

## Alternatives Considered

### A. Separate `StepUpLayer` middleware (not chosen)

A Tower layer wrapping entire routers would be simpler to implement but
coarser-grained — it cannot apply per-route `max_age` values and cannot
differentiate between mutating and read-only routes within the same router.
The attribute macro gives route-level granularity that matches `#[secured]`
ergonomics.

### B. Store claim as a typed `DateTime` (not chosen)

Storing the timestamp as a typed `chrono::DateTime` in the session would
require the session store to serialize/deserialize a non-primitive type.
Storing as a Unix timestamp string keeps the session layer generic and avoids
a dependency on `chrono` in the session KV abstraction.

### C. Separate session key per operation (not chosen)

Different operations could store `last_strong_auth_at_account_destroy`,
`last_strong_auth_at_mfa_disable`, etc. This would let each operation have its
own expiry without sharing the window. The complexity is not justified: the
`max_age` annotation on the route already allows per-operation windows, and
sharing a single claim mirrors the industry pattern (GitHub sudo mode, AWS
console) where a single elevation covers the session.

### D. JWT claims instead of session claims (not chosen)

JWTs cannot be revoked server-side. A step-up claim in a JWT would remain
valid until expiry even if the server decides to invalidate it. Session claims
are revocable via `session.destroy()` or backend eviction.

## Consequences

### Positive

- Single attribute (`#[step_up]`) provides declarative, auditable step-up
  protection with zero hand-written middleware.
- Audit events emit automatically; no per-handler instrumentation needed.
- Scaffold handles new apps on day one; existing apps add the attribute to
  existing handlers.
- JSON / browser branching is transparent to the handler author.

### Negative

- Handlers annotated with `#[step_up]` receive four injected extractors that
  do not appear in the function signature; this is the same trade-off as
  `#[secured]`.
- `last_strong_auth_at` is stored as a string (Unix timestamp) rather than a
  typed `DateTime`. Parsers must handle malformed values gracefully (the
  implementation treats parse failures as absence).
- The reauth redirect carries the path as a URL parameter. If the path
  contains user-controlled data, the `return_to` validation must remain
  rigorous (same-origin check is applied).

## Step-Up vs. Session Rotation

These two primitives address different threats and are both necessary:

| | `Session::rotate_id()` | `#[step_up]` |
|---|---|---|
| **Threat** | Session fixation | Hijacking / abandoned session |
| **When** | At login, at password reset | Before each sensitive handler |
| **User action** | None | Re-enter credentials |
| **Revokes old session** | Yes | No (reuses existing session) |

See `docs/guide/step-up-authentication.md` for usage guidance.

## References

- Issue #833 — original specification
- `autumn/src/step_up.rs` — core implementation
- `autumn-macros/src/step_up.rs` — proc macro
- `autumn-admin-plugin/src/auth.rs` — admin middleware
- `docs/guide/step-up-authentication.md` — end-user guide
