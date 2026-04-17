with open("autumn/src/telemetry.rs", "r") as f:
    content = f.read()
content = content.replace('.map(|value| value.eq_ignore_ascii_case("production"))\n        .unwrap_or(false)', '.is_ok_and(|value| value.eq_ignore_ascii_case("production"))')
with open("autumn/src/telemetry.rs", "w") as f:
    f.write(content)
