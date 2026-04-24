1. **Refactor `Schedule` to implement `std::fmt::Display`**
   - The formatting of `Schedule` is duplicated in several places in `autumn/src/app.rs`.
   - Implement `std::fmt::Display` for `autumn/src/task.rs`'s `Schedule`.
2. **Update usages in `autumn/src/app.rs`**
   - Update `app.rs` to use `Schedule`'s `Display` implementation instead of repeating `match` logic.
3. **Verify Tests**
   - Run `cargo test -p autumn-web` to ensure no behavior change.
   - Run `cargo fmt --all` and `cargo clippy --all-targets --all-features -- -D warnings`.
4. **Pre-commit Checks**
   - Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.
5. **Submit PR**
   - Create PR with '⚒️ Forge: [refactor name]' format.
