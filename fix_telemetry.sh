git checkout autumn/src/telemetry.rs
# Add allow(dead_code) to ExporterInit
sed -i 's/ExporterInit(String),/#[allow(dead_code)]\n    ExporterInit(String),/g' autumn/src/telemetry.rs
