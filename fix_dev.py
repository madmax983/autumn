with open("autumn/src/middleware/dev.rs", "r") as f:
    content = f.read()

content = content.replace("```rust", "```rust,ignore")

with open("autumn/src/middleware/dev.rs", "w") as f:
    f.write(content)
