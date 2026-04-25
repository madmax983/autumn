1. **Understand Codecov Failure**
   - The CI is failing because `codecov/patch` shows `76.47% of diff hit (target 90.00%)`.
   - This means that our newly added lines in `autumn/src/cache/layer.rs` and `autumn/src/middleware/metrics.rs` are not fully covered by tests.
2. **Identify Uncovered Code**
   - In `autumn/src/cache/layer.rs`, we added a fallback branch `super::get::<CachedResponse>(store.as_ref(), &format!("http:{}", req.uri()))` for when the URI is extremely long (exceeds 512 bytes).
   - We need to add a test in `autumn/src/cache/layer.rs` that explicitly calls the cache layer with an extremely long URI (e.g., > 512 chars) to trigger that fallback path.
3. **Write Tests to Increase Coverage**
   - Update `autumn/src/cache/layer.rs`'s test module to add a test case that makes a GET request with a very long URI and validates it correctly hits/misses the cache and doesn't panic.
   - For `autumn/src/middleware/metrics.rs`, we added a `if key_str.is_empty()` check. This happens when the `{method} {route}` string exceeds 256 bytes. We should also add a test there to pass a very long route to `record()`.
4. **Complete Pre-Commit Checks & Submit**
   - Run tests `cargo test -p autumn-web --lib cache::layer` and `cargo test -p autumn-web --lib middleware::metrics`.
   - Follow pre-commit checks and submit.
