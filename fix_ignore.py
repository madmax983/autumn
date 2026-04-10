with open("autumn/src/middleware/dev.rs", "r") as f:
    dev = f.read()

dev = dev.replace("```rust,ignore", "```rust")
dev = dev.replace("autumn_web::middleware::dev::is_enabled()", "autumn::middleware::dev::is_enabled()")

with open("autumn/src/middleware/dev.rs", "w") as f:
    f.write(dev)

with open("autumn/src/test_utils.rs", "r") as f:
    test_utils = f.read()

test_utils = test_utils.replace("```rust,ignore", "```rust")
test_utils = test_utils.replace("use autumn_web::test_utils::EnvGuard;", "use autumn::test_utils::EnvGuard;")

with open("autumn/src/test_utils.rs", "w") as f:
    f.write(test_utils)
