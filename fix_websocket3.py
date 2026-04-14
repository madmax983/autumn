with open('autumn/tests/websocket.rs', 'r') as f:
    content = f.read()

content = content.replace('autumn_web::router::build_router(routes![with_shutdown], &config, state)', 'autumn_web::test::TestApp::new().routes(autumn_web::routes![with_shutdown]).config(config).build().into_router()')
content = content.replace('let state = test_state();\n    let app', 'let _state = test_state();\n    let app')
content = content.replace('let state = test_state();\n    let router', 'let _state = test_state();\n    let router')

with open('autumn/tests/websocket.rs', 'w') as f:
    f.write(content)
