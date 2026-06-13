1. **Identify the smell:** The code in `autumn/src/experiments.rs` repeats the logic for extracting an exclusion group string from `ExperimentStoreError::Backend` messages that start with "ExcludedByGroup:". This duplication creates unnecessary boilerplate and reduces clarity.
2. **Extract common logic:** Create a helper function `parse_excluded_by_group(msg: &str) -> Option<String>` in `autumn/src/experiments.rs` that checks if the message starts with "ExcludedByGroup:" and extracts the group name.
3. **Refactor occurrences:** Use this helper function in `assign_with_request_id` (around line 1111 and 1166) to replace the duplicate block of logic.
4. **Complete pre-commit steps:** Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.
5. **Verify changes:** Use `cargo check -p autumn-web --lib`, `cargo clippy -p autumn-web --lib`, `cargo test -p autumn-web --lib experiments::`, and `cargo fmt --all` to verify the refactoring correctly retains the same logic and tests pass.
6. **Submit PR:** Submit the changes following the Forge persona PR conventions.
