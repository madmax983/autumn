# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] HTMX Headers and CRLF Injection
The `append_hx_header` function utilizes `HeaderValue::from_str`. The standard `http::HeaderValue` securely validates strings to ensure they contain no invalid characters like `\r` or `\n`. Attempted CRLF injections return a `Result::Err` during construction, and since `append_hx_header` simply ignores any `Err`, header injection via `hx_trigger` or `hx_location` is not possible.

# [ERIS-NOTE] HTMX Out-of-Band (OOB) Swap Injection
The Maud templating language inherently prevents HTML element injection via variable interpolation by implicitly escaping attributes like `hx-swap-oob`. Because `htmx` attribute processing relies on correct DOM insertion (not unescaped strings), XSS / DOM manipulation via user string interpolation into `hx-*` attributes is structurally impossible when using Maud templates.

# [ERIS-NOTE] HTMX Request Spoofing
While it is trivial to forge `HX-Request` headers, the only side effect is changing whether handlers render partial templates versus full pages. This does not grant access to restricted data or bypass CSRF protection. Therefore, `HX-Request` spoofing is not a vulnerability; it is standard HTTP content negotiation.
