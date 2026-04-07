with open("autumn/src/state.rs", "r") as f:
    content = f.read()

import re
test_re = re.compile(r'(#\[cfg\(test\)\]\nmod tests \{.*?\n\})\n*(#\[cfg\(feature = "db"\)\]\nimpl crate::db::DbState for AppState \{.*?\})', re.DOTALL)
match = test_re.search(content)

if match:
    tests = match.group(1)
    impl = match.group(2)

    # replace tests + impl with impl + tests
    content = content.replace(match.group(0), impl + "\n\n" + tests)

    with open("autumn/src/state.rs", "w") as f:
        f.write(content)
