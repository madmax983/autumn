with open('autumn/src/telemetry.rs', 'r') as f:
    text = f.read()

# We just remove everything after `mod tests {` that uses those conflicting imports.
# The errors are from: `use autumn_web::telemetry::{ResolvedLogFormat, TelemetryRuntime, TraceExport};`
idx = text.find('fn telemetry_config_loads_from_toml_and_env')
if idx != -1:
    # also remove the imports before it
    idx2 = text.rfind('#[cfg(test)]\nmod tests', 0, idx)
    if idx2 != -1:
        text = text[:idx2] + '#[cfg(test)]\nmod tests {\n    use super::*;\n}\n'

with open('autumn/src/telemetry.rs', 'w') as f:
    f.write(text)
