with open("autumn/src/middleware/dev.rs", "r") as f:
    content = f.read()

content = content.replace("autumn::middleware::dev::is_enabled()", "autumn_web::middleware::dev::is_enabled()")

with open("autumn/src/middleware/dev.rs", "w") as f:
    f.write(content)

with open("autumn/src/test_utils.rs", "r") as f:
    content = f.read()

content = content.replace("use autumn::test_utils::EnvGuard;", "use autumn_web::test_utils::EnvGuard;")

with open("autumn/src/test_utils.rs", "w") as f:
    f.write(content)
