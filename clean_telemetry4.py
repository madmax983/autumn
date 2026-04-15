with open('autumn/src/telemetry.rs', 'r') as f:
    text = f.read()

idx = text.rfind('#[cfg(test)]')
if idx != -1:
    text = text[:idx]

with open('autumn/src/telemetry.rs', 'w') as f:
    f.write(text)
