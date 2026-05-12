1. **Target:** Extract logic from the `run` function in `autumn/src/app.rs`.
   - **Smell:** `run` is a God Function (~360 lines) orchestrating mode checks, config loading, database setup, router building, task scheduling, and shutdown handling. It's difficult to follow the high-level execution flow.
   - **Solution:** I will use python scripts to extract the HTTP server binding, task execution, and graceful shutdown logic (around lines 1675-1770) into a new private helper function named `run_server_with_graceful_shutdown`. I will update the `run` function to call this helper instead.
2. Verify the changes by running the compiler and linter: `cargo clippy -p autumn-web --all-targets --all-features -- -D warnings` and formatting `cargo fmt --all`.
3. Verify the tests pass: `cargo test -p autumn-web --all-features` to ensure no runtime behavior has changed.
4. Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.
5. Create a PR with title `⚒️ Forge: [Extract server shutdown logic from app run]` and the required description fields: '🚮 Smell:', '✨ Solution:', '🧼 Benefit:', '🛡️ Verification:'.
