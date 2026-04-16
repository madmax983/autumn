import re

# Fix websocket tests using `TestApp`
with open('autumn/tests/websocket.rs', 'r') as f:
    content = f.read()

# Instead of `autumn_web::router::build_router(routes![...], &config, state);`
# Use `TestApp::new().routes(routes![...]).config(config).build().router`
# But wait, TestClient's router is private. We can just add `.router()` to `TestClient` or `.into_router()` to `TestApp` or just make `router` public in `TestClient`.

content = content.replace('autumn_web::router::build_router(routes![echo], &config, state)', 'autumn_web::test::TestApp::new().routes(autumn_web::routes![echo]).config(config).build().into_router()')
content = content.replace('autumn_web::router::build_router(routes![with_shutdown], &config, state)', 'autumn_web::test::TestApp::new().routes(autumn_web::routes![with_shutdown]).config(config).build().into_router()')

with open('autumn/tests/websocket.rs', 'w') as f:
    f.write(content)

# Add into_router to TestClient
with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

content = content.replace('pub fn get(&self, uri: &str) -> RequestBuilder {', 'pub fn into_router(self) -> axum::Router {\n        self.router\n    }\n\n    pub fn get(&self, uri: &str) -> RequestBuilder {')

with open('autumn/src/test.rs', 'w') as f:
    f.write(content)
