import re

with open("autumn-cli/src/monitor.rs", "r") as f:
    content = f.read()

content = re.sub(
    r'struct ConfigProperty',
    r'pub struct ConfigProperty',
    content
)

with open("autumn-cli/src/monitor.rs", "w") as f:
    f.write(content)
