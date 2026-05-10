# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] HTMX Out-of-Band (OOB) Injection
Extensive testing of Maud templates and Autumn's integration shows that Out-of-Band (OOB)
response injection is structurally impossible under normal conditions.

1. Maud escapes all user input provided to elements or attributes automatically.
2. `htmx_oob_envelope` specifically takes a well-typed `maud::Markup`, so developers
   cannot inject raw HTML strings directly into the OOB swap envelope without explicitly
   using `PreEscaped`.
3. User input inside `aria_live_region` is also properly escaped.
