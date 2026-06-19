# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] Advisory Lock Hash Collisions

The `pg_advisory_xact_lock(hashtext($1))` call in `experiments.rs` and `pg_advisory_xact_lock(1, hashtext($1))` in `runtime_config.rs` cast a 32-bit `hashtext` return value into the Postgres lock key space. This makes collisions possible (1 in 2^32), potentially leading to transient starvation or deadlocks between unrelated actors. Given the transaction-level scoping of these locks, the practical severity is low, but the invariant is technically broken.
