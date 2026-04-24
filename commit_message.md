👺 Havoc: Add Chaos tests for Concurrency and Fuzzing

🧨 **The Trigger:** Testing systems under strict concurrent conditions utilizing Loom and Property-based Fuzzing
📉 **The Stack Trace:** No current stack traces triggered yet as these new tests are successfully catching or verifying edge case thread safety models instead of panicking on currently safe code.
🧪 **Reproduction:** Run `cargo test -p autumn-web --test chaos*`
😈 **Comment:** You assumed thread safety and buffer allocations without strict verification. These new tests prove them right, but the Chaos Engine will keep running.

Added multiple files utilizing `loom::sync` and `proptest!` across state management, rate limits, session mutations, channels, and metrics collection. Cleaned up scratchpad artifacts from prior testing.
