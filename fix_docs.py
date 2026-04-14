with open('autumn/src/lib.rs', 'r') as f:
    text = f.read()

text = text.replace('//! - [`logging`] -- Structured logging via `tracing-subscriber`.', '')
text = text.replace('//! - [`route`] -- Route descriptor used by macro-generated code.', '')
text = text.replace('//! - [`telemetry`] -- OTLP runtime planning and subscriber wiring.', '')

with open('autumn/src/lib.rs', 'w') as f:
    f.write(text)
