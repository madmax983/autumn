import re

with open("autumn-cli/src/monitor.rs", "r") as f:
    content = f.read()

content = re.sub(
    r'#\[derive\(Debug, Deserialize, Default, Clone\)\]\npub type ConfigPropsResponse',
    r'pub type ConfigPropsResponse',
    content
)

with open("autumn-cli/src/monitor.rs", "w") as f:
    f.write(content)
