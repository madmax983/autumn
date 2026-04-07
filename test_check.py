with open("autumn/src/state.rs", "r") as f:
    state_content = f.read()

import re

# find tests module
test_re = re.compile(r'#\[cfg\(test\)\]\nmod tests \{.*\}', re.DOTALL)
match = test_re.search(state_content)

print(len(match.group(0)) if match else 0)
