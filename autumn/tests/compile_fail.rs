#[test]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();

    // Route macro failures (always available)
    t.compile_fail("tests/compile-fail/empty_path.rs");
    t.compile_fail("tests/compile-fail/missing_leading_slash.rs");
    t.compile_fail("tests/compile-fail/non_async.rs");
    t.compile_fail("tests/compile-fail/non_async_main.rs");
    t.compile_fail("tests/compile-fail/non_function.rs");
    t.compile_fail("tests/compile-fail/routes_nonexistent.rs");

    // Static route macro failures
    t.compile_fail("tests/compile-fail/static_get_path_params.rs");
    t.compile_fail("tests/compile-fail/static_get_non_async.rs");
    t.compile_fail("tests/compile-fail/static_get_params_no_placeholders.rs");

    // Model macro failures (require db feature)
    #[cfg(feature = "db")]
    t.compile_fail("tests/compile-fail/model_on_enum.rs");

    // Repository hooks failures (require db feature)
    #[cfg(feature = "db")]
    t.compile_fail("tests/compile-fail/repository_hooks_not_default.rs");

    // Cached macro failures
    t.compile_fail("tests/compile-fail/cached_self_receiver.rs");
    t.compile_fail("tests/compile-fail/server_action_non_async.rs");
}

#[test]
fn compile_pass_tests() {
    let t = trybuild::TestCases::new();

    // Route macro passes (always available)
    t.pass("tests/compile-pass/valid_handlers.rs");
    t.pass("tests/compile-pass/async_main.rs");
    t.pass("tests/compile-pass/static_get_basic.rs");
    t.pass("tests/compile-pass/static_routes_basic.rs");
    t.pass("tests/compile-pass/static_get_parameterized.rs");

    // Interceptor macro
    t.pass("tests/compile-pass/intercept_basic.rs");

    // Maud + form/json handlers (require maud feature)
    #[cfg(feature = "maud")]
    t.pass("tests/compile-pass/json_form_handlers.rs");

    // Model derive (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/model_derive.rs");

    // Model field enum (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/model_field_enum.rs");

    // Model draft accessors (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/model_draft_accessors.rs");

    // Repository compile-pass (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_no_hooks.rs");
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_with_hooks.rs");
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_with_api.rs");
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_with_hooks_and_api.rs");

    // Cached macro
    t.pass("tests/compile-pass/cached_basic.rs");
    t.pass("tests/compile-pass/cached_result.rs");
    t.pass("tests/compile-pass/server_action_basic.rs");
}
