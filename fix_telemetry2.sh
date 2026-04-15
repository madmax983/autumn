# The tests in telemetry.rs were likely added by me earlier and are broken.
# Let's remove the test block at the end of telemetry.rs if it's broken.
# Wait, I previously ran `cat autumn/tests/telemetry_config.rs >> autumn/src/telemetry.rs`! Let's just restore telemetry.rs and add the dead_code allow correctly.
git restore autumn/src/telemetry.rs
sed -i 's/ExporterInit(String),/#[allow(dead_code)]\n    ExporterInit(String),/g' autumn/src/telemetry.rs
