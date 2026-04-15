🧪 Sentinel: test coverage for signals

🎯 **What:** The `autumn-harvest` module `signal.rs` completely lacked any tests for `send_signal`, `load_pending_signals`, and `mark_signals_consumed`. A test file `signal_tests.rs` has been created.
📊 **Coverage:** E2E DB integration test added using `testcontainers`. Covers:
1. Signal queuing (`send_signal`) via foreign-keyed execution ID constraints.
2. Signal fetching (`load_pending_signals`) and state assertions.
3. Consumed marking (`mark_signals_consumed`) testing visibility scope.
✨ **Result:** Test coverage for `signal.rs` endpoints. All test logic was modeled via the `integration_e2e.rs` template ensuring full end-to-end integration matching production paths.
