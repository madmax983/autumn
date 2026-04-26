1. Break circular dependency between `db` and `state`.
   - `db` imported `AppState` in tests. We changed this to use a local `MockState` that implements `DbState`, preventing `db` from depending on `state`. This is completed.
2. Break circular dependency between `actuator` and `state`.
   - `actuator` imported `AppState` in tests. We changed this to use a local `MockActuatorState` that implements `ProvideActuatorState` and `ProvideProbeState`, preventing `actuator` from depending on `state`. This is completed.
3. Verify changes.
   - Run `find_cycles.py` again. It should output no cycles involving these.
   - Run `cargo clippy --all-targets --all-features -- -D warnings`.
   - Run `cargo test --workspace`.
   - All pass.
4. Clean up scripts and files.
5. Create a PR.
