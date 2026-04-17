🔒 Warden: [Fix potential data exposure in mask_database_url fallback]

🦠 **Threat:**
The `mask_database_url` function parses database URLs using the `url` crate to securely redact passwords before logging. However, if an attacker or misconfiguration provides a malformed URL (e.g., one containing spaces or invalid characters) that still contains a password, `url::Url::parse` will fail. The previous implementation fell back to logging the unparsed, raw URL string, exposing the password in plaintext logs.

🛡️ **Defense:**
Updated the fallback logic in `mask_database_url`. If `url::Url::parse` fails, the function now safely redacts the entire URL string (`"****"`). This ensures a "fail-closed" mechanism, preventing any sensitive information from leaking into the logs when the parser encounters malformed input. Valid URLs without passwords remain unaffected and are logged naturally.

💥 **Severity:**
High. Exposure of database credentials in logs can lead to unauthorized database access, data breaches, and system compromise.

🧪 **Verification:**
Added the unit test `mask_database_url_invalid_url_fallback` in `autumn/src/app.rs` to verify that malformed URLs containing a password are fully redacted and do not expose the password. Verified that `cargo test`, `cargo clippy`, and `cargo fmt` pass successfully.
