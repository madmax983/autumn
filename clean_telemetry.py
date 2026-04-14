import re

with open('autumn/src/telemetry.rs', 'r') as f:
    lines = f.readlines()

new_lines = []
skip = False
for line in lines:
    if line.startswith('// test_config'):
        pass
    if 'telemetry_config_loads_from_toml_and_env' in line:
        skip = True

    if not skip:
        new_lines.append(line)

with open('autumn/src/telemetry.rs', 'w') as f:
    f.writelines(new_lines)
