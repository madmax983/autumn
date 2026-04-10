1. **Analyze Target (`autumn-harvest/autumn-harvest/src/worker.rs`)**:
   - `HandlerRegistry::fmt` calls `.keys().collect::<Vec<_>>()` unnecessarily. The `.keys()` iterator implements `Debug` out of the box so we can just use `&self.workflows.keys()` and `&self.activities.keys()`. This removes allocations every time `HandlerRegistry` is formatted with Debug (like in logs or errors).
   - In `find_pending_activity` (lines 468-486).
     ```rust
     let mut pending = history
         .iter()
         .filter_map(|event| match event { ... });

     match (pending.next(), pending.next()) {
         (Some(activity_id), None) => Ok(*activity_id),
         (None, _) => Err(...),
         (Some(_), Some(_)) => Err(...),
     }
     ```
     This collects an intermediate `Vec<_>` just to check if it has exactly 1 element. We can avoid this allocation entirely by using an iterator and checking `.next()`.

2. **Run Tests**:
   - Verify `cargo clippy`, `cargo fmt`, and `cargo test -p autumn-harvest --lib` all pass.

3. **Pre-commit**:
   - Ensure proper verification.

4. **Submit PR**:
   - Use Bolt persona PR format:
     - 💡 What: Avoided intermediate `.collect::<Vec<_>>()` allocations in `worker.rs`.
     - 🎯 Why: They allocated memory unnecessarily on the hot path (like checking pending activities and formatting registries).
     - 📊 Impact: Removes heap allocations per check.
     - 🔬 Measurement: Code review & tests passing.
