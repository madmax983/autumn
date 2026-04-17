with open("autumn/src/logging.rs", "r") as f:
    content = f.read()
content = content.replace('.map(|v| v.eq_ignore_ascii_case("production"))\n        .unwrap_or(false)', '.is_ok_and(|v| v.eq_ignore_ascii_case("production"))')
with open("autumn/src/logging.rs", "w") as f:
    f.write(content)
