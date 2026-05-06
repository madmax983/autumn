Everything is green!

1. **Verify code quality:**
   - Ran `cargo check`, `cargo test`, `cargo fmt` and `cargo clippy`.
   - Addressed all structural issues, testing, and linting.
2. **Submit PR:**
   - Pre-commit: Call `pre_commit_instructions`.
   - Submit: Use the submit tool with the Atlas persona fields:
     - Title: "🗺️ Atlas: [Break state/authorization/session circular dependency]"
     - Description:
       🕸️ Tangle: The `state` -> `authorization` -> `session` -> `state` structural knot due to module entanglement.
       📐 Blueprint: We introduced `ProvideAuthorizationState` trait to invert dependencies. We refactored `apply_session_layer` to take `Router<S>` generic. We removed explicit imports to `AppState` from tests to clean the module hierarchy. Finally, we adjusted the `autumn-macros` to emit inferred bounds `_` so that generic type resolution properly passes.
       🧱 Stability: Reduced tight coupling to the God Struct `AppState`, eliminating a circular dependency cycle.
       🔭 Verification: Passes full `cargo test -p autumn-web --all-targets --all-features` and local module-level verification. Cleanly builds without regressions.
