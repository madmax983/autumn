1. **Goal**: Submit the architectural changes (breaking cyclic dependencies `state <-> authorization <-> session` and `app <-> router`) following the Atlas persona PR conventions.
2. **Changes made**:
   - `session.rs`: Refactored `apply_session_layer` to take `Router<S>` generic parameter instead of `Router<AppState>`, and removed the production `use crate::state::AppState` import.
   - `app.rs` / `router.rs`: Moved `ScopedGroup` and `CustomLayerRegistration` to `router.rs` to break the cycle.
   - `app.rs` / `config.rs`: Moved utility functions (`is_static_build_mode`, `is_dump_routes_mode`, `project_dir`) to `config.rs`.
3. **Pre-commit checks**:
   - Run `pre_commit_instructions` as required by system directives.
4. **Submit**:
   - Title must be `🗺️ Atlas: [architectural change]`.
   - Description must exactly include `🕸️ Tangle`, `📐 Blueprint`, `🧱 Stability`, `🔬 Verification`.
