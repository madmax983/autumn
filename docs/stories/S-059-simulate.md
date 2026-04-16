# 🌟 Nova: Traffic Simulator

## 💡 The Spark
Autumn currently lacks a quick, integrated way to load test or generate traffic against a running instance. Often, developers want to see how their application handles load or want to populate monitoring endpoints (like `/actuator/metrics`) to observe real-time data in `autumn monitor`. Having a built-in traffic generator directly in the `autumn` CLI enables easy experimentation without reaching for heavy external tools like `wrk` or `hey`.

## 🚀 The Feature
Implemented a new CLI command `autumn simulate` (or simply `autumn sim`) that spins up multiple concurrent workers using the existing `reqwest` client. It continuously hits the application URL for a specified duration or until interrupted, generating significant HTTP traffic.
- Subcommand: `autumn simulate`
- Arguments: `--url` (target URL, defaults to `http://localhost:3000`), `--workers` (number of concurrent threads, defaults to 10), and `--duration` (duration to run in seconds).

## 🔮 The Potential
Could be used for generating load for performance testing, validating rate limiting configurations, populating Prometheus metrics for the monitor dashboard, or triggering concurrency bugs in development.

## ⚠️ Risk
Low. Isolated entirely within the CLI `autumn-cli/src/simulate.rs` and has no impact on the framework core logic. Uses the already imported `reqwest` crate for blocking HTTP requests in separate threads.
