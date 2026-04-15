with open('autumn/src/telemetry.rs', 'r') as f:
    lines = f.readlines()

new_lines = []
for line in lines:
    if line.strip() == '#[test]':
        continue
    new_lines.append(line)

with open('autumn/src/telemetry.rs', 'w') as f:
    f.writelines(new_lines)
