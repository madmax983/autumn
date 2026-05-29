# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-FINDING] X-Forwarded-* Spoofing Bypass in Method Override
- **Threat:** Attacker can spoof `X-Forwarded-Host` and `X-Forwarded-Proto` headers (e.g., by prepending their own values) to bypass Origin/Host CSRF validations in the Method Override middleware.
- **Severity:** High (CSRF bypass)
- **Note:** After analysis, blindly parsing `.last()` breaks legitimate multi-proxy setups. A proper fix requires edge-proxy header stripping or configuring `trusted_proxies`.
- **Proof of Concept:** Added `eris_attacker_can_spoof_x_forwarded_host_and_proto` to `autumn/tests/security/method_override_spoofing.rs` to track this vulnerability.
