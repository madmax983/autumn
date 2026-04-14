with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

content = content.replace(
    'use crate::router::build_router;',
    ''
)

with open('autumn/src/test.rs', 'w') as f:
    f.write(content)

with open('autumn/src/router.rs', 'r') as f:
    content = f.read()

content = content.replace(
    'pub fn build_router(',
    '#[allow(dead_code)]\npub fn build_router('
)

with open('autumn/src/router.rs', 'w') as f:
    f.write(content)
