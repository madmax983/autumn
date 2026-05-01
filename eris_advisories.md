# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] HTMX Out-of-Band Swaps (hx-swap-oob)

The hypothesis that "response injection modify unrelated DOM elements via hx-swap-oob" was tested. The analysis shows that out-of-band swaps require the `hx-swap-oob` attribute to be rendered in the HTML response. Because Maud compiles templates at build time and escapes all dynamic input by default (unless explicitly wrapped in `PreEscaped`), it is structurally impossible for an attacker to inject arbitrary HTML attributes like `hx-swap-oob="true"` into the response, or to inject entire out-of-band response tags. All HTMX headers generated server-side are strictly typed and do not include mechanisms for unescaped HTML injection. Therefore, this attack vector is not exploitable in the current architecture.
