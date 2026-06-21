1. Extract `constant_time_eq_str` and `constant_time_eq` into `autumn/src/security/constant_time.rs`.
   - `constant_time_eq` (for `&[u8]`) is used in `pagination.rs` and `storage/local.rs`.
   - `constant_time_eq_str` (for `&str`) is used in `security/csrf.rs`.
   - They currently duplicate the same constant-time loop but adapt to their type.
   - Refactor `constant_time_eq_str` to wrap `constant_time_eq(a.as_bytes(), b.as_bytes())`.
2. Add `pub mod constant_time;` to `autumn/src/security/mod.rs`.
3. Use the extracted functions across the codebase instead of the duplicate `fn constant_time_eq` blocks.
   - Replace in `autumn/src/pagination.rs`.
   - Replace in `autumn/src/security/csrf.rs`.
   - Replace in `autumn/src/storage/local.rs`.
   - Replace the two `unwrap_u8()` patterns in `autumn/src/auth.rs` which misuse `ct_eq` on slices directly by using the centralized `constant_time_eq_str`.
4. Ensure `cargo clippy`, `cargo test`, and `cargo fmt` pass without warnings or errors.
5. Provide a PR named "⚒️ Forge: [refactor constant_time_eq]"
