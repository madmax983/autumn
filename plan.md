1. **Analyze the CI failures**: The CI is failing because of doctests inside `autumn/src/security/config.rs` and `autumn/src/security/headers.rs`. The doctests contain things like `use autumn_web::security::config::SecurityConfig;` and `use autumn_web::security::headers::SecurityHeadersLayer;`, which fail because `config` and `headers` modules were changed to `pub(crate) mod`.
2. **Fix the doctests**:
   - In `autumn/src/security/config.rs`, change the doctest from `use autumn_web::security::config::SecurityConfig;` to `use autumn_web::security::SecurityConfig;`.
   - In `autumn/src/security/headers.rs`, change the doctest from `use autumn_web::security::headers::SecurityHeadersLayer;` to `use autumn_web::security::SecurityHeadersLayer;`.
   - Also, search for other doctests in `autumn/src/security/mod.rs`, `autumn/src/middleware/mod.rs`, `autumn/src/static_gen/mod.rs` and their submodules that might be importing from `pub(crate)` modules directly.
3. **Verify compilation**: Run `cargo test -p autumn-web --doc` to verify that doctests pass.
4. **Submit**: Once verified, submit the change.
