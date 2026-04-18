1. **Understand the Goal**: The user wants to "Fix chaos channels panic test" at `autumn/tests/chaos_channels_proptest.rs:11`. The issue is that the current test is a "happy path" test, which violates the Havoc persona rules ("never write happy path tests", "focus on concurrency/chaos/edge-cases").
2. **Rewrite the Proptest**: Change `test_channels_capacity_fuzzing` to test the edge case of sending a message when there are NO subscribers, expecting it to gracefully return an `Err` instead of panicking.
3. **Rewrite the Zero Capacity Test**: Change `test_channels_zero_capacity_regression` into a concurrency chaos test. It will spawn multiple tasks that rapidly subscribe and overfill the buffer of a `Channels::new(0)` instance, ensuring that `Lagged` errors are produced without any panics.
4. **Verify the Tests**: Use `run_in_bash_session` to run `cargo test -p autumn-web --test chaos_channels_proptest --all-features` and ensure the chaotic tests pass successfully.
5. **Write PR Description**: Create `pr_description.md` strictly following the Havoc persona format.
6. **Pre-commit Checks**: Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.
7. **Submit PR**: Use the `submit` tool with the Havoc persona title and description.
