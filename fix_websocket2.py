with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

content = content.replace(
    '#[must_use]\n    pub fn into_router(self) -> axum::Router {',
    'pub fn into_router(self) -> axum::Router {'
)

with open('autumn/src/test.rs', 'w') as f:
    f.write(content)

with open('autumn/tests/probe_contracts.rs', 'r') as f:
    content = f.read()

content = content.replace(
    'let mut config = AutumnConfig::default();\n    config.health = HealthConfig',
    '#[allow(clippy::field_reassign_with_default)]\n    let mut config = AutumnConfig::default();\n    config.health = HealthConfig'
)

with open('autumn/tests/probe_contracts.rs', 'w') as f:
    f.write(content)
