with open('autumn/tests/session_backends.rs', 'r') as f:
    text = f.read()

text = text.replace(
    'let app1 = router::build_router(\n        routes![write_session, read_session],\n        &config,\n        AppState::for_test().with_pool(pool.clone()),\n    );',
    'let app1 = TestApp::new().routes(autumn_web::routes![write_session, read_session]).config(config.clone()).with_db(pool.clone()).build().into_router();'
)

text = text.replace(
    'let app2 = router::build_router(\n        routes![write_session, read_session],\n        &config,\n        AppState::for_test().with_pool(pool.clone()),\n    );',
    'let app2 = TestApp::new().routes(autumn_web::routes![write_session, read_session]).config(config).with_db(pool.clone()).build().into_router();'
)

with open('autumn/tests/session_backends.rs', 'w') as f:
    f.write(text)
