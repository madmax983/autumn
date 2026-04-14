with open('autumn/tests/session_backends.rs', 'r') as f:
    text = f.read()

# Replace uses of `router::build_router` with `TestApp` in `session_backends.rs`
# Or better, just rewrite the problematic function `session_layer_persists_data_across_requests`!
# Let's see what it does.
text = text.replace('use autumn_web::route::Route;', 'use autumn_web::Route;')
text = text.replace('use autumn_web::router;', 'use autumn_web::test::TestApp;')

# The test uses `router::build_router(routes![write_session, read_session], &config, AppState::for_test().with_pool(pool.clone()))`
text = text.replace(
    'let app1 = router::build_router(\n        routes![write_session, read_session],\n        &config,\n        AppState::for_test().with_pool(pool.clone()),\n    );',
    'let app1 = TestApp::new().routes(routes![write_session, read_session]).config(config.clone()).with_db(pool.clone()).build().router;'
)
text = text.replace(
    'let app2 = router::build_router(\n        routes![write_session, read_session],\n        &config,\n        AppState::for_test().with_pool(pool.clone()),\n    );',
    'let app2 = TestApp::new().routes(routes![write_session, read_session]).config(config).with_db(pool.clone()).build().router;'
)

# And fix TestClient visibility to make `router` accessible or just use TestClient's methods.
# Wait, `TestClient::router` is private. So let's use `TestApp`'s `.get()` and `.post()` methods!

with open('autumn/tests/session_backends.rs', 'w') as f:
    f.write(text)
