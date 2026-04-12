import re

with open("autumn-cli/src/monitor.rs", "r") as f:
    content = f.read()

content = re.sub(
    r'assert_eq!\(state\.active_tab, 1\);',
    r'assert_eq!(state.active_tab, 2);',
    content
)

with open("autumn-cli/src/monitor.rs", "w") as f:
    f.write(content)
