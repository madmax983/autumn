🔒 Warden: [security fix] Add harvest_api_with_auth for management API protection

🦠 Threat
The `autumn-harvest` management API (`/api/harvest`) was previously only mountable via `.harvest_api("/api/harvest")` which did not provide any native way to apply authentication middleware. By default, this exposed endpoints for enumerating DAGs, starting workflows, and triggering DAG runs, which could allow an unauthenticated attacker to manipulate background tasks and internal state (CWE-306).

🛡️ Defense
Introduced a new method `harvest_api_with_auth<M>` to the `HarvestExt` trait. This allows developers to mount the management API protected by a custom `tower::Layer` (such as `autumn_web::auth::RequireAuth`), enforcing authentication before requests reach the internal API router.

💥 Severity
High. Unauthenticated access to a workflow management API allows arbitrary triggering of tasks, potentially leading to unauthorized data modification, business logic bypass, or Denial of Service (DoS).

🧪 Verification
Created `eris_authenticated_harvest_api_start_workflow` PoC test in `autumn-web-harvest/tests/security.rs` to verify that when the API is mounted using `harvest_api_with_auth` and a `RequireAuth` middleware, unauthenticated requests are appropriately blocked with a `401 Unauthorized` status code.
