with open("autumn/src/state.rs", "r") as f:
    content = f.read()
content = content.replace('.map(|extensions| extensions.len())\n                .unwrap_or(0)', '.map_or(0, |extensions| extensions.len())')
with open("autumn/src/state.rs", "w") as f:
    f.write(content)
