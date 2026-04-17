🧹 Code Health: Fix Unused Variables in Metrics Endpoint

🎯 **What:** Removed the `#[allow(unused_variables, unused_mut)]` attribute from the `metrics_endpoint` function in `autumn/src/actuator.rs` and properly scoped the mutability of the `result` binding so it only uses `mut` when the `db` feature is enabled.
💡 **Why:** Suppressing compiler warnings with `#[allow(...)]` attributes hides potentially useful signals and clutters the code. By structurally scoping the mutability based on the feature flags (`#[cfg(feature = "db")]`), we eliminate the unused mutability warning naturally. This improves maintainability and ensures the code accurately reflects its intent under different compilation profiles.
✅ **Verification:**
- Verified `cargo check` and `cargo check --no-default-features` pass without unused mutability warnings.
- Verified `cargo clippy --all-targets --all-features -- -D warnings` passes.
- Verified `cargo test -p autumn-web --all-features metrics` passes successfully.
- Verified that the `result` binding is not modified if the `db` feature is not active.
✨ **Result:** Clean, warning-free metrics endpoint that correctly configures mutability and removes unnecessary global suppression attributes.
