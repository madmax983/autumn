🦠 Threat: RUSTSEC-2026-0009 in `time` dependency. Denial of Service via Stack Exhaustion.
🛡️ Defense: Updated `time` dependency in `Cargo.toml` to `>=0.3, <0.4` to resolve the vulnerability (bumps to `0.3.47`). Note that this requires Rust 1.88+, so I updated `rust-version` in `Cargo.toml` to `1.88.0`. Also included updated `Cargo.lock`.
💥 Severity: 6.8 (medium)
🧪 Verification: Ran `cargo audit` to confirm vulnerability is resolved.
