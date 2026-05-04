# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] Maud HTML Attribute Injection Bypass

The hypothesis that "I can break out of an HTML attribute context even within Maud's escaping" was tested and found to be false. The `html!` macro escapes attribute values using `&quot;`, preventing injection payloads like `"><script>alert(1)</script>` from escaping the attribute context.

# [ERIS-NOTE] HTMX Header Injection

The hypothesis that "forged HTMX headers can be injected to change server behavior or redirect users" was investigated. The framework utilizes Axum's `HeaderValue::from_str` for setting HTMX headers like `HX-Location` or `HX-Redirect`, which correctly rejects any string containing CRLF characters. Consequently, HTTP response splitting and header injection are not possible through these interfaces.

# [ERIS-NOTE] Session and CSRF Cookie Tossing

The hypothesis that "an attacker could toss a malicious session or CSRF cookie" was extensively tested. The `SessionLayer` and `CsrfLayer` both securely reject requests that present multiple cookies with the same name, mitigating cookie tossing attacks.

# [ERIS-NOTE] Authentication Timing and DoS Attacks

The hypotheses regarding timing attacks on the `verify_password` function and resource exhaustion via heavy bcrypt hashing blocking async worker threads were tested and found to be false. `verify_password` properly executes dummy hashes on invalid input to prevent timing attacks, and `hash_password` uses `tokio::task::spawn_blocking` to prevent blocking the async worker threads, effectively mitigating DoS vectors.

# [ERIS-NOTE] Path Traversal and Fallback Middleware Bypass

The hypotheses concerning path traversal (`/api/../submit`) to bypass CSRF middleware exempt lists and rate-limit middleware bypasses on fallback routes were verified as false. Axum's routing handles paths correctly, preventing traversal, and `TestApp` integrations verified that rate-limit and other global middlewares apply correctly across the application hierarchy.
