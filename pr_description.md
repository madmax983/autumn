🔒 Warden: [security fix] Remove unsafe process environment mutations in tests

🎯 **What:** The `EnvGuard` pattern in tests used `unsafe { std::env::set_var }` to modify process-wide environment variables, which can lead to data races and undefined behavior in multithreaded test environments. The vulnerability was discovered in `autumn/src/app.rs`, `autumn/src/middleware/dev.rs`, `autumn/src/wasm/mod.rs`, and `autumn-cli/src/dev.rs`.
⚠️ **Risk:** Modifying the environment concurrently can trigger data races and potentially result in severe vulnerabilities or undefined behavior, compromising the testing integrity.
🛡️ **Solution:** Replaced `EnvGuard` in the test suites with `MockEnv`, and updated production code (such as the dev middleware and wasm loaders) to accept a `dyn Env` parameter. For `autumn-cli/src/dev.rs`, test environments now correctly construct configurations instead of mutating globals.
