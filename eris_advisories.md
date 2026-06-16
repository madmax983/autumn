# [ERIS-NOTE] CSRF on HTMX Endpoints

The hypothesis that "I can bypass CSRF protection on htmx endpoints by omitting the HX-Request header and submitting a standard form POST" was extensively tested and found to be false. The `CsrfLayer` securely applies validation on all non-safe methods regardless of the presence of htmx-specific headers, and gracefully falls back to checking the URL-encoded body if the header token is absent.

# [ERIS-NOTE] Unresolved Dependencies Vulnerabilities

The following vulnerabilities reported by `cargo audit` have been investigated but cannot be safely resolved without significant architectural changes or upstream fixes:

1. **RUSTSEC-2023-0071 (rsa)**
   - **Threat**: Marvin Attack: potential key recovery through timing sidechannels in `rsa 0.9.10`.
   - **Constraint**: There is no fixed upgrade available for `rsa 0.9.10` that doesn't cause downstream breakage. It is a transitive dependency of `jsonwebtoken` which does not currently support `rsa 0.10.x`.

2. **RUSTSEC-2026-0098, RUSTSEC-2026-0099, RUSTSEC-2026-0104 (rustls-webpki)**
   - **Threat**: Validation and panic vulnerabilities in certificate parsing in `rustls-webpki 0.101.7`.
   - **Constraint**: This crate is pulled in via the `aws-sdk-s3` dependency tree. Updating `aws-smithy-http-client` to the required secure version forces the workspace `rust-version` requirement to jump from `1.88.0` to `1.91.1`, violating the project's current toolchain constraints.

3. **RUSTSEC-2026-0173 (proc-macro-error2)**
   - **Threat**: `proc-macro-error2` 2.0.1 is unmaintained.
   - **Constraint**: This crate is pulled in via `validator_derive`, which is essential to the project's input validation (`validator 0.20.0`). Upgrading `validator` to a newer version that does not depend on this is currently blocked due to cascading API breaks.

4. **RUSTSEC-2026-0138 (diesel-async)**
   - **Threat**: Unsound access to padding bytes while serializing date/time values using the Mysql backend in `diesel-async 0.8.0`.
   - **Constraint**: Upgrading `diesel-async` to `0.9.1` introduces a breaking change to the `AsyncConnection::transaction` method signature involving `AsyncFnOnce` and complex closure lifetimes. Attempting the upgrade caused unresolvable compiler errors in `autumn/src/job.rs`.
