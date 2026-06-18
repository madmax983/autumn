1. **Define `ProvideAuthorizationState` Trait:**
   - In `autumn/src/authorization.rs` (or an appropriate module), define `pub trait ProvideAuthorizationState: Send + Sync` that encapsulates the required methods.
   - Methods: `auth_session_key(&self) -> &str`, `policy_registry(&self) -> &crate::authorization::PolicyRegistry`, `forbidden_response(&self) -> &crate::authorization::ForbiddenResponse`.
   - Under `#[cfg(feature = "db")]`: `pool(&self) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>`.

2. **Implement `ProvideAuthorizationState` for `AppState`:**
   - In `autumn/src/state.rs`, add `impl crate::authorization::ProvideAuthorizationState for AppState`.

3. **Update `authorization.rs` functions:**
   - Modify the signature of functions taking `&crate::AppState` to take `&dyn ProvideAuthorizationState` or `&impl ProvideAuthorizationState` (or similar depending on turbofish restrictions).
   - *Self-correction based on memory:* "use dynamic dispatch (e.g., `&dyn Trait`) instead of `impl Trait` or adding a new named generic parameter. Using `impl Trait` in the argument list triggers error E0632 on stable Rust, and adding a named parameter breaks the original turbofish arity." So we will use `&dyn ProvideAuthorizationState`.

4. **Run Verification and Clippy:**
   - Ensure the new abstractions don't break existing tests.
   - Run tests for `autumn-web` (`cargo test -p autumn-web`).
   - Run `cargo clippy --all-targets --all-features -- -D warnings`.

5. **Commit and Submit PR:**
   - Title: "🗺️ Atlas: [architectural change]"
   - Description containing: `🕸️ Tangle`, `📐 Blueprint`, `🧱 Stability`, `🔬 Verification`.
