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

    // Model macro failures (require db feature)
    #[cfg(feature = "db")]
    t.compile_fail("tests/compile-fail/model_on_enum.rs");

    // Repository hooks failures (require db feature)
    #[cfg(feature = "db")]
    t.compile_fail("tests/compile-fail/repository_hooks_not_default.rs");
}

#[test]
fn compile_pass_tests() {
    let t = trybuild::TestCases::new();

    // Route macro passes (always available)
    t.pass("tests/compile-pass/valid_handlers.rs");
    t.pass("tests/compile-pass/async_main.rs");
    t.pass("tests/compile-pass/static_get_basic.rs");
    t.pass("tests/compile-pass/static_routes_basic.rs");

    // Maud + form/json handlers (require maud feature)
    #[cfg(feature = "maud")]
    t.pass("tests/compile-pass/json_form_handlers.rs");

    // Model derive (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/model_derive.rs");

    // Model field enum (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/model_field_enum.rs");

    // Repository compile-pass (requires db feature)
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_no_hooks.rs");
    #[cfg(feature = "db")]
    t.pass("tests/compile-pass/repository_with_hooks.rs");
}
