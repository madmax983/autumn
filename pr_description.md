# 🧪 [testing improvement description]

## 🎯 Target
The `Builder` queue configuration API (`WorkerConfig::with_queues`) lacked boundary edge case tests and enforcement to prevent misconfiguring the worker queue listener. Previously, it allowed injecting empty queue sets or invalid string vectors containing only empty strings, which could result in a configuration bug.

## 💣 Risk
Worker processes misconfigured with empty `queues` will not subscribe to anything and remain effectively broken and idle. This can be problematic if a configuration management tool or dynamic service sets it to an empty or whitespace string array, resulting in silent failures without any error or warning.

## 🧪 Strategy
1. **Implementation Upgrade:** Enforce robustness directly in the `WorkerConfig::with_queues` function. Using `.filter(|q| !q.trim().is_empty())` protects against whitespace-only configurations. Furthermore, checking if the resulting `new_queues` slice is empty ensures that if an empty array or purely empty string sequence is passed in, the `WorkerConfig` simply ignores the invalid update and maintains its fallback default `["default"]` configuration.
2. **Test Assertions:** Added three new edge-case tests validating this behavior:
   - `worker_config_with_empty_queues_ignores_update`
   - `worker_config_with_whitespace_queues_ignores_update`
   - `worker_config_with_mixed_queues_filters_empty`

## 🔭 Verification
Locally confirmed via test suite execution with `cargo check` and `cargo test builder`. Tests correctly demonstrate the robust rejection of empty configuration states.
