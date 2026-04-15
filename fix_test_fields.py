with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

content = content.replace(
    '    pub fn from_router(router: axum::Router) -> TestClient {\n        TestClient {\n            router,\n            url: "http://localhost:3000".to_owned(),\n            #[cfg(feature = "db")]\n            pool: None,\n        }\n    }',
    '    pub const fn from_router(router: axum::Router) -> TestClient {\n        TestClient {\n            router,\n        }\n    }'
)
content = content.replace(
    '/// Can be called multiple times -- routes are combined additively.\n\n    #[must_use]',
    '/// Can be called multiple times -- routes are combined additively.\n    #[must_use]'
)

with open('autumn/src/test.rs', 'w') as f:
    f.write(content)
