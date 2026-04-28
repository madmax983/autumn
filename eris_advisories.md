# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.
