1. **Objective**
   - Address the security vulnerability where Axum fallback routes bypass security middlewares (like CSRF, CORS, Rate Limiting, and Upload middlewares).
   - This occurs because `router.fallback(...)` was called AFTER `.layer(...)` applications in `autumn/src/router.rs`. In Axum, middleware added via `.layer()` only applies to routes and fallbacks defined *before* that call.

2. **Actions**
   - **Update `autumn/src/router.rs`**: Reorder the middleware application so that `router.fallback(crate::middleware::error_page_filter::fallback_404_handler)` is called *before* `apply_cors_middleware`, `apply_csrf_middleware`, `apply_rate_limit_middleware`, and `apply_upload_middleware`.
   - **Add Regression Test**: Ensure `autumn/tests/security/fallback_middleware_bypass.rs` tests the fallback handler correctly. The test will expect `429 Too Many Requests` on the second request to a 404 route, preventing a rate limit DoS vulnerability.
   - **Register the Regression Test**: Add `pub mod fallback_middleware_bypass;` to `autumn/tests/security/mod.rs`.

3. **Verify**
   - Run `cargo test -p autumn-web --test security_tests fallback_middleware_bypass` to verify the fix works.
   - Run `cargo clippy -p autumn-web --all-targets` and `cargo fmt --all`.

4. **Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.**
   - Run `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, and `cargo fmt --all` to make sure all standards are adhered to.
