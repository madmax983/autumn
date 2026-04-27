🦠 Threat:
An attacker could bypass security middlewares (including Rate Limiting, CSRF, CORS, and Upload protections) by requesting an unmapped route (e.g., `/does-not-exist`). This occurs because `router.fallback(...)` was called after applying these middleware layers in `autumn/src/router.rs`. In Axum, `.layer()` only wraps routes and fallbacks that are currently on the router. Bypassing the Rate Limiter via the fallback handler opens the application to a Denial of Service (DoS) attack, as handling 404 responses still consumes CPU and connection resources.

🛡️ Defense:
Reordered the middleware application in `autumn/src/router.rs`. The 404 fallback handler (`router.fallback(...)`) is now applied *before* the security middleware layers (CORS, CSRF, Rate Limit, Upload). This ensures that any request falling through to the 404 handler is still properly protected and rate-limited.

💥 Severity:
Medium (CVSS: 5.3) - Allows unauthenticated attackers to bypass the application's global rate limiting protections, potentially leading to resource exhaustion (DoS) through heavy 404 request spamming.

🧪 Verification:
Added `eris_fallback_middleware_bypass` inside `autumn/tests/security/fallback_middleware_bypass.rs`. The test creates a rate-limited router, sends two requests to a missing route (`/not-found`), and asserts that the second request correctly receives a `429 Too Many Requests` response instead of being blindly processed as a `404 Not Found`. All tests (`cargo test -p autumn-web`) pass successfully.
