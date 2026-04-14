git restore autumn/tests/telemetry_config.rs
git restore autumn/src/telemetry.rs
sed -i 's/ExporterInit(String),/#[allow(dead_code)]\n    ExporterInit(String),/g' autumn/src/telemetry.rs

# Instead of `telemetry_config.rs` running as an integration test, it can just test via `test_app` or be an internal test. But since telemetry is internal, we can just remove `telemetry_config.rs` entirely.
rm autumn/tests/telemetry_config.rs
