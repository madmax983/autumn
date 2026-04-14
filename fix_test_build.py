with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

content = content.replace(
    'let router = build_router(self.routes, &self.config, state);',
    'let router = crate::router::try_build_router_merged(self.routes, &self.config, state, self.merge_routers, self.nest_routers).unwrap();'
)

with open('autumn/src/test.rs', 'w') as f:
    f.write(content)
